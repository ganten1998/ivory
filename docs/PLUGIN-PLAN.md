# Tangent plugin (VST3 format): the implementation plan

Everything below was checked against the tree and against the dependency sources on this machine, not against the proposals. Where the proposals or judges were wrong, the correction is stated inline so nobody re-derives it.

---

## 0. Verified facts that decide the design

| Claim | Verdict |
|---|---|
| 14 `show_viewport_immediate` sites | **FALSE. Five.** `menu.rs:569`, `menu.rs:670`, `dialogs.rs:257`, `chord_strip.rs:230`, `fretboard_panel.rs:599`. All five already route through `shell::viewport_ui`. The sixth grep hit is a doc comment. |
| 11 `send_viewport_cmd` sites | **FALSE. Nine.** `app.rs` 478/1212/1233/1234/1235/1241/1242, `chord_strip.rs:259`, `fretboard_panel.rs:626`. |
| "the 8 dialogs" | **11 `Dialog` variants**, all through one funnel, `dialogs.rs:229 show_dialog_viewport`. Three of them (`MidiPicker`, `NoMidiInput`, `MidiError`) are midir artefacts and do not exist in the plugin, which is where "8" comes from. |
| `nih_plug_egui` pins egui 0.31 | **True of nih-plug's copy, and irrelevant.** `nih_plug_egui` v0.1.0 now lives *inside* BillyDM/egui-baseview. Verified at `~/.cargo/git/checkouts/egui-baseview-fde77becaa39f49a/11f487f/nih_plug_egui/Cargo.toml`: `egui-baseview = { path = "../" }`, parent is egui-baseview 0.7.0 on **egui 0.33**. No fork, no `[patch]`, no vendoring. |
| `EguiState::set_requested_size` is private, so the editor cannot resize | **Half true and the wrong conclusion.** It is private (`lib.rs`, `fn` not `pub fn`), but `nih_plug_egui::resizable_window::ResizableWindow` is **public** and calls it. And I traced egui 0.33's `CentralPanel::show_dyn` → `set_clip_rect(panel_rect)` → `Frame::begin`'s `new_child` does **not** narrow the clip rect, so `ui.clip_rect()` inside `ResizableWindow`'s closure is the **full editor rect**, and the `ui.new_child(max_rect: ui_rect)` it hands the body is full-bleed. **The plugin editor is resizable, and the body still gets the whole rect.** Every proposal and every judge got this wrong. |
| `show_viewport_immediate` fails in a plugin | **FALSE and dangerous.** `context.rs:735` sets `embed_viewports: true`; `context.rs:3951` returns `viewport_ui_cb(self, ViewportClass::Embedded)` inline. It runs, re-enters `viewport_ui`, and opens a second `CentralPanel` under `Id::new((ctx.viewport_id(), "central_panel"))` — the identical id (`panel.rs:1150`). Silent garbage over the piano. **The seam must sit above `shell::viewport_ui`.** |
| `ViewportCommand::InnerSize` is ignored in a plugin | **FALSE.** `egui-baseview/src/window.rs:370` calls `window.resize()`. Only `Close` and `InnerSize` are honoured; everything else is `_ => {}`. `app.rs:1235` would resize the host's child window behind its back on frame one, because `last_sent_size` starts `None`. Must be gated, not merely left to no-op. |
| `KeyCapture` default | **`CaptureAll`** (`egui-baseview/src/window.rs:91-94`, `#[default]`). An untreated plugin eats the DAW's spacebar. `Queue::set_key_capture` is public (`window.rs:60`). |
| `MidiCCs` cost | **2080 parameters.** `VST3_MIDI_CCS = 130` × `VST3_MIDI_CHANNELS = 16`, `nih-plug/src/wrapper/vst3/util.rs`. `MidiConfig::Basic` has no CC64, so no sustain. |
| nih-plug rev | Must be **exactly** `28b149ec4d62757d0b448809148a0c3ca6e09a95` (what `11f487f`'s `[workspace.dependencies]` pins). A different rev of the same URL = two `nih_plug` packages = "expected Editor, found Editor". It is already in `~/.cargo/git/db`, so the first build resolves offline. Their side takes it `default-features = false` (dropping `vst3`); we must enable `vst3` ourselves. |
| `gen-third-party-licenses.sh` | Runs `cargo license --json` at the workspace root, unfiltered, with `skip = {"ivory", "ivory-core"}` hardcoded. **If `ivory-plugin` is a root workspace member, the MIT standalone ships a licence manifest listing GPLv3 `vst3-sys`.** This is the single strongest argument for the quarantine. |
| `a_submenu_near_the_bottom_is_pulled_back_onto_the_screen` (menu.rs:896) | **Tests a local copy of the clamp, not `menu.rs`.** The clamp is currently unguarded. |
| `process_midi_events` (app.rs:269-303) | **Zero test coverage.** `midi.rs` tests `parse_message` and `pick_auto_port` only. |
| `Settings::save_to` (settings.rs:434-442) | Bare `fs::write`. `load_from` returns **all defaults** on any parse error. `OverrideStore::save` is already tmp+rename (`overrides.rs:262-268`) with a no-debris test. |
| Test count | **213** `#[test]`, of which 48 in `ivory/src/*.rs` and 1 in `ivory/tests/`. |
| `target/` | Already a symlink to `~/Library/Caches/ivory-target` because Dropbox breaks build-script linking. The plugin needs the same. |

---

## 1. Crate layout

**Five crates, two workspaces.** Every crate in the repo is MIT source; only the produced `.vst3` binary is GPL-3.0-or-later.

### Root workspace (`Cargo.toml`)

```toml
[workspace]
members  = ["ivory-core", "ivory-ui", "ivory"]
exclude  = ["plugin"]
resolver = "2"
```

**`ivory-core`** — LIB, MIT. Untouched except for `OverrideStore::save_merged` (step 9).

**`ivory-ui`** — NEW LIB (rlib), MIT. The shared GUI. Deps: `egui 0.33` (default-features off), `ivory-core = { path = "../ivory-core", features = ["learning", "license"] }`, `serde`, `serde_json`, `dirs`, `log`. Dev-dep `ttf-parser 0.25`.

Received by `git mv` from `ivory/src/`, **with no renames**: `app.rs`, `piano.rs`, `chord_strip.rs`, `fretboard_panel.rs`, `menu.rs`, `dialogs.rs`, `fonts.rs`, `settings.rs`, `shell.rs`. Plus new `notes.rs` (holding `MidiEvent`, `parse_message` and the `MidiPorts` trait) and new `inline_shell.rs`.

`IvoryApp` keeps its name. `app.rs` keeps its name. Renaming the file that carries the D-UI-11 geometry machinery costs `git log -L` continuity on the code with the AeroSpace 853x1377 story attached, for nothing.

Explicitly **not** dependencies: `eframe`, `midir`, `rfd`, `fd-lock`, `windows-sys`. This is the enforcement mechanism, and it is stronger than any feature flag: `#![windows_subsystem]`, `process::exit`, `rfd` modals and the single-instance lock are unreachable from shared code because the compiler cannot see them. **No `#[cfg(feature = ...)]` anywhere in `ivory-ui`.** Both host paths compile in every build.

> Why not P1's `[lib]` + `[[bin]]` in one package behind a `desktop` feature: it puts the first `#[cfg]` into shared GUI code, which is the seed of the two-GUI fork that the 0.33 pin exists to prevent (HANDOFF §2c), and it relies on nobody ever feature-unifying eframe back in. The three hazards this costs us (see below) are one-line fixes.

Three consequences to handle in the same commit that creates the crate:
- `gen-third-party-licenses.sh`: `skip = {"ivory", "ivory-core", "ivory-ui"}` in **both** the cargo-license and the `cargo metadata` branch (the latter already filters by `workspace_members`, so only the first needs it).
- `dialogs.rs:442`'s `env!("CARGO_PKG_VERSION")` now reads `ivory-ui`'s version. `ivory-ui` uses `version.workspace = true`, so it is the same string. Add `pub const VERSION: &str = env!("CARGO_PKG_VERSION");` in `ivory-ui/src/lib.rs` and have About use it, so the coupling is named rather than incidental.
- The `features = ["learning", "license"]` line **moves** to `ivory-ui/Cargo.toml`, and `ivory/tests/license_feature.rs` moves to `ivory-ui/tests/license_feature.rs`. Keep a copy in `ivory/tests/` as well. This is the trap that has already bitten once.

`include_bytes!("../../assets/fonts/...")` still resolves: `ivory-ui/src` is at the same depth as `ivory/src`.

**`ivory`** — BIN, MIT, package name and `[[bin]] name = "tangent"` unchanged. Keeps `main.rs`, `midi.rs` (midir half + `pick_auto_port`), `build.rs`. Gains `desktop_shell.rs` (`ViewportShell`, `DesktopApp`, `impl eframe::App for DesktopApp`).

The orphan rule forces the wrapper: `impl eframe::App for ivory_ui::IvoryApp` is illegal in crate `ivory`. So:

```rust
// ivory/src/desktop_shell.rs
pub struct DesktopApp { app: IvoryApp, shell: ViewportShell }
impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().frame(egui::Frame::NONE)
            .show(ctx, |ui| self.app.paint(ui, &mut self.shell));
    }
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] { [0.0, 0.0, 0.0, 1.0] }
}
```

Two disjoint fields, so the borrow works.

### Quarantined workspace (`plugin/Cargo.toml`)

```toml
# DELIBERATELY OUTSIDE the main workspace (note the empty [workspace] table).
#
# This is the structural guarantee that NIH-plug's GPLv3 VST3 bindings can
# never reach the MIT standalone:
#   - `cargo build -p ivory-plugin` from the repo root cannot resolve it,
#   - nih_plug / vst3-sys / baseview never enter the root Cargo.lock,
#   - so they never appear in THIRD-PARTY-LICENSES or any release artifact.
# It is a promise enforced by cargo, not by discipline.
[workspace]
members = ["ivory-plugin", "xtask"]
```

That comment is the one already in `tools/ivory-keygen/Cargo.toml`, with `vst3-sys` in place of `ed25519-compact`. It is the repo's own established mechanism for exactly this problem.

**`ivory-plugin`** — `crate-type = ["cdylib"]`. Source MIT (`license = "MIT"` written literally, not inherited, with a header comment stating the artifact is GPL-3.0-or-later).

```toml
[dependencies]
ivory-ui   = { path = "../../ivory-ui" }
ivory-core = { path = "../../ivory-core", features = ["learning", "license"] }
egui       = { version = "0.33", default-features = false }
# The rev MUST equal the one nih_plug_egui pins (11f487f's [workspace.dependencies]).
# A different rev of the same URL builds TWO nih_plug crates and you get
# "expected Editor, found Editor". Their side takes it default-features = false,
# which drops `vst3`; we add it back, and that line is where GPLv3 attaches.
nih_plug      = { git = "https://github.com/robbert-vdh/nih-plug.git",
                  rev = "28b149ec4d62757d0b448809148a0c3ca6e09a95",
                  default-features = false, features = ["vst3"] }
nih_plug_egui = { git = "https://github.com/BillyDM/egui-baseview.git",
                  rev = "11f487fe915d6208961064e619474b98f971594a" }
crossbeam-queue = "0.3"
parking_lot     = "0.12"
serde_json      = "1"
```

Keep `nih_plug_egui`'s default features (`opengl`, `default_fonts`). `default_fonts` reaches `egui/default_fonts`, which is what `fonts::install` builds its fallback chain from; without it the submenu arrow `U+23F5` and the bullet `U+2022` render as tofu.

**`xtask`** — three lines wrapping `nih_plug_xtask::main()`. Plus `plugin/bundler.toml`:

```toml
[ivory-plugin]
name = "Tangent"
```

Cross-workspace path deps are fine. `plugin/Cargo.lock` is committed and is the GPL "exact revision" record.

**Dependency direction, asserted in CI:** `ivory-plugin → ivory-ui → ivory-core`, and `ivory → ivory-ui → ivory-core`. Never the reverse. Nothing depends on `ivory-plugin`.

```bash
# in scripts/build-plugin.sh and in the release script
cargo metadata --format-version 1 --locked | grep -q nih_plug && exit 1   # root is clean
cargo tree -p ivory-ui | grep -Eq 'eframe|midir|rfd|fd-lock' && exit 1    # ui is clean
grep -rn 'process::exit\|rfd::' ivory-ui/src ivory-core/src && exit 1     # no DAW killers
grep -rn 'send_viewport_cmd' ivory-ui/src && exit 1                       # no host commands
```

**`ivory-ui` must stay an rlib.** Never `dylib`, never `cdylib`. A shared dynamic library between the two binaries is the one thing the GPLv3 §5 aggregation argument cannot survive. Add this line to LICENSING.md.

---

## 2. `Shell` and `Caps`

All of this lives in `ivory-ui/src/shell.rs`. `viewport_ui` stays exactly as it is, tests included: it is still the root bridge for all three entry points, and its test is what proves a `CentralPanel` did not gain a margin.

```rust
// ── capabilities ───────────────────────────────────────────────────────────
//
// Field polarity is chosen so DESKTOP is all-true and PLUGIN is all-false.
// A test asserts exactly that, so a ninth capability cannot silently default
// to off on the desktop or on in the plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// We can create real OS windows. False => surfaces are drawn inline.
    pub own_windows: bool,
    /// Detached chord window and popped-out neck exist (HANDOFF §2c).
    pub detachable: bool,
    /// Borderless/Bordered, Decorations, StartDrag are ours.
    pub window_chrome: bool,
    /// Geometry write-back, the offscreen rescue and the tiling-WM guard run.
    pub remembers_geometry: bool,
    /// We choose our own size (Min+Max+Inner triple, Size submenu).
    /// False => we lay out into the rect we were given.
    pub app_sets_size: bool,
    /// We enumerate and open MIDI ports ourselves.
    pub own_midi_ports: bool,
    /// One process, one instance: the lock file and the welcome dialog.
    pub single_instance: bool,
    /// ~/.config/ivory/settings.json is ours to write.
    pub owns_settings_file: bool,
}

impl Caps {
    pub const DESKTOP: Caps = Caps {
        own_windows: true, detachable: true, window_chrome: true,
        remembers_geometry: true, app_sets_size: true, own_midi_ports: true,
        single_instance: true, owns_settings_file: true,
    };
    pub const PLUGIN: Caps = Caps {
        own_windows: false, detachable: false, window_chrome: false,
        remembers_geometry: false, app_sets_size: false, own_midi_ports: false,
        single_instance: false, owns_settings_file: false,
    };
}

// ── what a surface asks for ────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SurfaceKey { Menu, Submenu, Dialog, ChordWindow, Fretboard }

pub struct SurfaceSpec<'a> {
    pub key: SurfaceKey,
    /// Window title. Ignored by InlineShell except as the drawn title-bar text.
    pub title: &'a str,
    pub size: egui::Vec2,
    pub min_size: egui::Vec2,
    /// Top-left in Shell::bounds() space. None = let the host centre it.
    pub pos: Option<egui::Pos2>,
    pub decorated: bool,
    pub resizable: bool,
    pub always_on_top: bool,
    /// The submenu passes false: it must not steal key focus from the menu.
    pub takes_focus: bool,
    /// Backdrop that swallows input to everything beneath. Dialogs only.
    pub modal: bool,
}

/// Everything the body used to read out of `i.viewport()`. Those five reads
/// mean nothing in an inline surface (they return the ROOT viewport's values,
/// silently wrong rather than failing), so they are hoisted up to here.
#[derive(Clone, Copy, Default, Debug)]
pub struct SurfaceReport {
    /// Title-bar close, Escape, or a click outside. One meaning, two sources.
    pub dismissed: bool,
    pub focused: Option<bool>,
    pub inner_size: Option<egui::Vec2>,
    pub outer_pos: Option<egui::Pos2>,
    /// Right-click inside this surface, in bounds space.
    pub context_menu_at: Option<egui::Pos2>,
}

/// What the body wants done to its surface. `ivory-ui` contains zero
/// `send_viewport_cmd` calls; this is the only way out.
#[derive(Clone, Copy, Default)]
pub struct SurfaceOut { pub start_drag: bool }

// ── the seam ───────────────────────────────────────────────────────────────
pub trait Shell {
    fn caps(&self) -> Caps;

    /// The rect everything is positioned and clamped inside, in the same
    /// coordinates as `SurfaceSpec::pos`.
    ///   desktop: (0,0)..monitor, or Rect::EVERYTHING while unknown
    ///   plugin:  the editor's own rect
    /// This one method deletes every `monitor: Option<Vec2>` in the codebase.
    fn bounds(&self) -> egui::Rect;

    /// Widget-local -> bounds space. Desktop adds the window's inner origin;
    /// the plugin is the identity.
    fn to_bounds(&self, local: egui::Pos2) -> egui::Pos2;

    /// THE method. Draw `add` into a surface described by `spec`.
    ///
    /// INVARIANT, tested for both implementors: `add` is handed a `Ui` whose
    /// `max_rect` is exactly the (clamped) pane rect and whose clip rect is
    /// the same. Nothing else. Every drawing body in this app paints
    /// absolutely into `max_rect`, so one source serves both hosts.
    fn surface(
        &mut self,
        ctx: &egui::Context,
        spec: &SurfaceSpec<'_>,
        add: &mut dyn FnMut(&mut egui::Ui, SurfaceReport, &mut SurfaceOut),
    ) -> SurfaceReport;

    /// Desktop: the Min+Max+Inner triple behind its `last_sent_size` latch.
    /// Only ever called under `caps().app_sets_size`.
    fn request_root_size(&mut self, ctx: &egui::Context, size: egui::Vec2);
    /// Desktop: Decorations + Title, behind its `decorations_sent` latch.
    fn set_decorations(&mut self, ctx: &egui::Context, decorated: bool);
    fn start_root_drag(&mut self, ctx: &egui::Context);
    fn nudge_root_on_screen(&mut self, ctx: &egui::Context, to: egui::Pos2);

    /// Called once per frame at the end of `paint` with
    /// `ctx.wants_keyboard_input()`. Desktop: no-op. Plugin: drives
    /// `Queue::set_key_capture`, which is the difference between a good
    /// citizen and a plugin that eats the DAW's spacebar.
    fn keyboard_wanted(&mut self, wanted: bool);

    /// The ONLY path from `ivory-ui` to persisted settings. Desktop writes
    /// settings.json; the plugin writes its own VST3 state and never the file.
    fn persist(&mut self, settings: &Settings);
}
```

### The one shared layout helper

```rust
/// Give `add` a `Ui` of exactly `natural` size, scrolling inside `ui`'s pane
/// when the pane is smaller. On the desktop the pane is always exactly
/// `natural`, so this is the identity and no pixel moves. In the plugin it is
/// how a 545pt menu and a 460pt dialog live in a 200pt editor without any
/// body knowing.
pub fn fit_or_scroll<R>(
    ui: &mut egui::Ui,
    natural: egui::Vec2,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let pane = ui.max_rect();
    if pane.height() + 0.5 >= natural.y && pane.width() + 0.5 >= natural.x {
        return add(ui);
    }
    egui::ScrollArea::both().auto_shrink([false; 2]).show(ui, |ui| {
        let rect = egui::Rect::from_min_size(ui.max_rect().min, natural);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let r = add(&mut child);
        ui.allocate_rect(rect, egui::Sense::hover());
        r
    })
}
```

Two call sites: the menu's row loop (after it paints its own background over `ui.max_rect()`), and `show_dialog_viewport`'s content call. Because the inner child gets `max_rect == natural`, the dialogs' `ui.add_space((ui.available_height() - 30.0).max(0.0))` button-push idiom and `MidiPicker`'s `available_height() - bottom_h` list sizing keep working unchanged. **Zero dialog bodies change.**

### `ViewportShell` (in `ivory/src/desktop_shell.rs`)

```rust
pub struct ViewportShell {
    inner_origin: Pos2, origin_known: bool, monitor: Option<Vec2>,
    last_sent_size: Option<Vec2>, decorations_sent: Option<bool>,
    // menu.rs's 250ms focus grace lives here now, where the concept exists.
    menu_opened_at: Option<Instant>, saw_focus: bool,
}
```

`surface()` is today's code moved, not rewritten: build a `ViewportBuilder` from `SurfaceSpec` (the field list above is exactly the union of what `menu.rs:557`, `menu.rs:659`, `dialogs.rs:239`, `chord_strip.rs:220` and `fretboard_panel.rs:589` set), `ctx.show_viewport_immediate(ViewportId::from_hash_of(id_for(spec.key)), builder, |vp, _| viewport_ui(vp, |ui| add(ui, report, &mut out)))`, fill `SurfaceReport` from `ui.input(|i| i.viewport())`, apply `out.start_drag` as `ViewportCommand::StartDrag`.

Ids stay byte-identical: `"ivory-menu"`, `"ivory-menu-sub"`, `"ivory-dialog"`, `"ivory-chord-window"`, `"ivory-fretboard-window"`.

`bounds()` = `Rect::from_min_size(Pos2::ZERO, mon)` when the monitor is known, `Rect::EVERYTHING` otherwise. `to_bounds(p)` = `inner_origin + p.to_vec2()`.

### `InlineShell` (in `ivory-ui/src/inline_shell.rs`)

It lives in the library, not in the plugin crate, so it is testable headlessly and reachable from the desktop binary under `IVORY_INLINE=1`.

```rust
pub struct InlineShell {
    pub bounds: Rect,
    pub caps: Caps,
    pub dark: bool,
    /// Read back by the caller after `paint`.
    pub key_capture_wanted: bool,
    pub settings_dirty: Option<Settings>,
    menu_dismissed: bool,
}

fn surface(&mut self, ctx, spec, add) -> SurfaceReport {
    let size = spec.size
        .min(self.bounds.size() - vec2(16.0, 16.0))
        .max(spec.min_size.min(self.bounds.size()));
    let pos = spec.pos.map(|p| settings::clamp_to_bounds(p, size, self.bounds))
        .unwrap_or_else(|| self.bounds.center() - size * 0.5);
    let rect = Rect::from_min_size(pos, size);
    let mut report = SurfaceReport {
        focused: Some(true), inner_size: Some(size), outer_pos: Some(pos),
        ..Default::default()
    };
    let mut out = SurfaceOut::default();

    if spec.modal {
        // Dialogs. egui::Modal gives backdrop + input blocking + Escape +
        // click-outside for free, and `dialog: Option<Dialog>` means there is
        // never more than one, so there is never more than one modal layer.
        let area = egui::Modal::default_area(Id::new(spec.key))
            .fixed_pos(rect.min).constrain_to(self.bounds);
        let r = egui::Modal::new(Id::new(spec.key))
            .area(area)
            .frame(egui::Frame::NONE)                       // bodies paint their own bg
            .backdrop_color(Color32::from_black_alpha(96))
            .show(ctx, |ui| {
                let mut c = ui.new_child(UiBuilder::new().max_rect(rect));
                c.set_clip_rect(rect);
                add(&mut c, report, &mut out);
            });
        report.dismissed = r.should_close();
    } else {
        // Menu and submenu: plain Areas, deliberately NOT Modals.
        // Two nested Modals would make the submenu the top modal layer and
        // the menu would then never see a click-outside. One Area each, with
        // the submenu above, plus explicit click-outside detection, is both
        // simpler and an exact match for the desktop.
        let order = if spec.key == SurfaceKey::Submenu {
            egui::Order::Tooltip } else { egui::Order::Foreground };
        egui::Area::new(Id::new(spec.key)).order(order)
            .fixed_pos(rect.min).constrain_to(self.bounds)
            .show(ctx, |ui| {
                let mut c = ui.new_child(UiBuilder::new().max_rect(rect));
                c.set_clip_rect(rect);
                add(&mut c, report, &mut out);
            });
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        let outside = ctx.pointer_interact_pos().is_none_or(|p| !rect.contains(p));
        report.dismissed = esc || (spec.key == SurfaceKey::Menu && pressed && outside);
    }
    report
}
```

**Click-through, which nobody costed.** On the desktop the menu is its own window and `handle_main_interaction` (app.rs:429-440) closes it on `primary_pressed` and `return`s, so the click does *not* land on a key. Inline, the same thing happens for free: `handle_main_interaction` still returns early while `menu_state.is_some()`, and `menu_state` is only cleared later in `paint()` by `menu::show`. Identical behaviour, zero extra code. For dialogs, `Modal`'s backdrop senses `CLICK | DRAG` (verified, `modal.rs:94-98`) and swallows the click; keep app.rs:415's `if self.dialog.is_some() { return; }` as well, belt and braces.

### `HeadlessShell` (in `ivory-ui/tests/`)

An `InlineShell` over `egui::Context::default()` at a fixed `bounds`, plus a recording fake that captures every `SurfaceSpec`. This is a real gain: `menu::show` and `dialogs::show` cannot be unit-tested today because they create OS windows.

**Static assertion, in `ivory-ui/src/lib.rs`:**

```rust
// nih_plug_egui::create_egui_editor requires T: 'static + Send. Without this
// the failure is a wall of trait errors in the plugin crate, not here.
// (Send only, NOT Sync: the 0.7 adapter relaxed it, which is what lets
// IvoryApp keep its mpsc::Receiver.)
const _: () = { fn a<T: Send>() {} fn _c() { a::<crate::app::IvoryApp>(); } };
```

---

## 3. What each module changes

| File | Change |
|---|---|
| `app.rs` | `IvoryApp::new(ctx: &egui::Context, caps: Caps, settings: Settings) -> Self` (loses `cc`, loses `cli_port`, loses the midir call). New `pub fn midi_sender()`, `pub fn set_ports(Box<dyn MidiPorts>)`, `pub fn paint(&mut self, ui, shell: &mut dyn Shell)`. Nine `send_viewport_cmd` → `shell.*`. `monitor_size` field deleted in favour of `shell.bounds()`. Eleven guard points (below). |
| `menu.rs` | `MenuState.monitor: Option<Vec2>` → `bounds: Rect`. `MenuState::open(ctx, view, caps, at, bounds)`. Two viewport calls → `shell.surface`. The five `i.viewport()` reads → `SurfaceReport`. Row loop wrapped in `fit_or_scroll`. `build_entries(view, caps)` gains four guards. |
| `dialogs.rs` | `Placement` deleted; `show(ctx, shell, dialog_opt, dark_mode, parent: Option<Rect>)`. `show_dialog_viewport` gains `shell` as its first arg, one `fit_or_scroll` call, and a drawn title bar when `!caps.own_windows`. **All 11 bodies unchanged.** |
| `chord_strip.rs`, `fretboard_panel.rs` | One `shell.surface` call each, `StartDrag` → `out.start_drag`, `i.viewport()` reads → `SurfaceReport`. Only reachable under `caps.detachable`. |
| `piano.rs`, `fonts.rs`, `settings.rs` | Untouched except `clamp_to_monitor` → `clamp_to_bounds`. |
| `notes.rs` (new) | `MidiEvent` (+ `AllNotesOff`), `parse_message` + its test, `trait MidiPorts`. |

### The eleven guard points in `app.rs`, exhaustively

Each is `if shell.caps().x { <existing code verbatim> }`, no restructuring inside, so review confirms them by indentation against this list.

1. `new()` L164-166 + L180-181: both `startup_*_detach_at` gated on `detachable`.
2. `new()` L171: `let welcome = (caps.single_instance && settings.show_welcome).then(...)`. **This one is a trap, not a nicety.** `handle_main_interaction` returns early while `self.dialog.is_some()` (L415-417), and `show_welcome` defaults true, so gating only the *render* leaves the plugin with a piano that is permanently dead to clicks and no visible cause. Gate the **creation**.
3. `new()` L158-161: the midir connect leaves `IvoryApp` entirely; `ivory/src/main.rs` does it and calls `app.set_ports(...)`.
4. `paint()` L1146-1183: both startup-restore blocks, `detachable`.
5. `paint()` L1190-1207: the `i.viewport()` inner/outer/monitor read and `main_inner_origin` tracking, `remembers_geometry`. Otherwise `main_inner_origin = ui.max_rect().min; main_origin_known = true;`.
6. `paint()` L1208-1214: the offscreen rescue, `remembers_geometry`.
7. `paint()` L1221-1226: `main_live_pos` / `geometry_save_at` tracking, `remembers_geometry`.
8. `paint()` L1228-1236: the Min/Max/Inner triple → `shell.request_root_size(ctx, target)`, gated on `app_sets_size`. **This is the one that must not merely no-op:** `ViewportCommand::InnerSize` really is honoured by egui-baseview, so an ungated call resizes the host's editor on frame one.
9. `paint()` L1238-1244: Decorations + Title → `shell.set_decorations`, gated on `window_chrome`.
10. `paint()` L1293-1376: both popout blocks, `detachable`. L1380-1410: geometry write-back, `remembers_geometry`.
11. `handle_main_interaction` L477-479: `StartDrag` → `shell.start_root_drag`, gated on `window_chrome`.

Plus, not a guard but a branch: `layout_sizes()`.

```rust
fn band_sizes_for_width(w: f32, settings: &Settings) -> (f32, f32, f32, f32) { /* today's body, minus main_width */ }
fn band_sizes(settings: &Settings) -> (f32, f32, f32, f32) {
    band_sizes_for_width(main_width(settings), settings)
}
// in paint():
let (w, piano_h, chord_h, fret_h) = if shell.caps().app_sets_size {
    self.layout_sizes()
} else {
    band_sizes_for_width(ui.max_rect().width(), &self.settings)
};
```

Purely additive. `size_math_matches_python_int_truncation` and `the_fretboard_band_joins_the_stack_at_every_size` stay green and **unedited**. If either needs editing, the change is wrong.

### `build_entries` under `Caps::PLUGIN`

Dropped: `Size` submenu (`!app_sets_size`), `Borderless/Bordered` (`!window_chrome`), `Select MIDI Input...` (`!own_midi_ports`), `Detach/Attach Chord Window` and `Detach/Attach Fretboard` (`!detachable`).
Added: `Save Appearance as Default` when `!owns_settings_file`.
Everything else survives verbatim: colours, dark mode, font cycle, supporter key, heart, keytoggle, sharps/flats, chord detection, teach, manage taught, correct, chord learning, fretboard toggle, wood, tuning, capo, About, Reset.

`settings.window_size_percent` is **never** clamped and written back. In the plugin it is derived from the actual rect for display only. Writing a clamp back would silently resize the user's desktop window forever.

---

## 4. Menu and dialogs in the plugin

The desktop **does not change**. `ViewportShell` is and remains the desktop default; the same OS windows appear at the same monitor positions with the same title bars, taskbar entries and close boxes.

### The menu

`menu.rs`'s only real change is `monitor: Option<Vec2>` → `bounds: Rect`. The existing clamp (menu.rs:470-479) and the existing submenu slide-up/flip-left (menu.rs:641-658) then serve the editor rect with **zero new geometry code**. The desktop passes `Rect(0,0..monitor)` so nothing moves; the plugin passes the editor rect, so a menu opened near the bottom-right of the editor behaves exactly as it does against a monitor edge. This is the single cheapest high-leverage change in the plan.

One correction that must go in with it: `clamp_to_monitor` (settings.rs:533-542) has a hard-coded `0.0` floor. `clamp_to_bounds` must clamp to `bounds.min`, or a second monitor at negative coordinates starts snapping windows to the primary display's origin, which the settings loader explicitly allows for. Keep the "unknown means unknown" semantics by returning `pos` unchanged when `bounds` is not finite; the existing `a_dialog_centres_on_the_window_or_declines_to_place_itself` test guards a bug that was found on a user's screen.

**Height.** The menu is 22 items + 13 separators at `row_h ≈ 23` = about **545pt**, 637pt with the fretboard block, against a 200pt editor. menu.rs's own header already says the 200px window "cannot host a ~460px menu". The answer in v1 is `fit_or_scroll`: a vertical `ScrollArea` around the row loop, engaged on **geometry** (`pane.height() < state.size.y`), not on host identity, so the desktop takes the same branch if a monitor is ever that small. **Column reflow is not v1.** Reasoning in one line: reflow is new UI with its own break-point rules, its own submenu re-anchoring and its own test surface, and it is desktop-unreachable by construction so it rots; a scrolled menu is three lines. And because the editor is resizable (§5), a user who dislikes scrolling can drag the editor taller and the menu fits outright.

**Focus.** The 250ms `opened_at` / `saw_focus` / all-unfocused close logic is viewport-specific and moves into `ViewportShell`. `InlineShell` reports `dismissed` from Escape or a press outside the menu rect. Both paths ship, both are tested, neither is deleted.

### The dialogs

`show_dialog_viewport` is the only function that changes, and the 11 bodies do not.

- Under `ViewportShell`: today's code, byte for byte, including `with_always_on_top` (dialogs.rs:245 documents that a modal dialog behind the main window is "an app that has stopped responding with no visible reason why") and `with_active(true)`.
- Under `InlineShell`: an `egui::Modal` with a `from_black_alpha(96)` backdrop, `Frame::NONE`, and the pane rect from `Placement`'s existing centring arithmetic (which needs no change, only `parent = Some(shell.bounds())` as input, so the "declines to place itself" case disappears in the plugin because the editor rect is always known). Then, inside the pane, `show_dialog_viewport` carves 24pt off the top for a drawn title bar (title left, `✕` right, themed from `menu::colors(dark_mode)`, about 20 lines) and calls `fit_or_scroll` on the rest with the dialog's declared natural size.

Eight dialogs in the plugin: `About`, `Welcome` (suppressed by Caps, so effectively seven reachable), `SupporterKey`, `ColorPick`, `TeachChord`, `ManageTaught`, `CorrectChord`, `LearnResult`. `MidiPicker`, `NoMidiInput` and `MidiError` are unreachable because there is no port to pick.

`stock_style()` (dialogs.rs:186) reads `i.raw.system_theme`, which egui-baseview does not populate, so it always resolves light. All three of its users are MIDI dialogs, which the plugin does not have. Nothing to do; written down so nobody "fixes" it later.

### `IVORY_INLINE=1`, and why it comes before the plugin crate

The standalone gets a developer switch that swaps `ViewportShell` for `InlineShell` (`IVORY_INLINE=1`), plus `IVORY_CANVAS=1300x200` to force the layout to believe it has a plugin-sized rect. The entire plugin UI then renders inside the shipping desktop binary, where it can be screenshotted and pixel-checked with the same decode-to-BMP run-counting method that found D-UI-17's twelve one-pixel slivers, **before nih-plug has ever been compiled**. Every taste judgment about a scrolled menu and a scrimmed dialog happens there, cheaply, and the inline path is exercised by the desktop build forever so it cannot rot.

---

## 5. Size and the editor rect

Verified above: `nih_plug_egui::resizable_window::ResizableWindow` is public, calls the private `set_requested_size`, and hands its closure a `Ui` whose `max_rect` is the **full** editor rect (`CentralPanel::show_dyn` sets the clip to `panel_rect`; `Frame::begin`'s `new_child` does not narrow it; `ResizableWindow` reads `ui.clip_rect()` and builds its child from that). So:

- The plugin's update closure is `ResizableWindow::new("tangent").min_size(min).show(ctx, &egui_state, |ui| app.paint(ui, &mut shell))`.
- `min` = `band_sizes_for_width(650.0, &settings)`, i.e. the 50% stack.
- Initial size = `initial_window_size(&settings)`, so the plugin opens at the size the user's standalone uses.
- The app lays out into `ui.max_rect().width()` (`!caps.app_sets_size`), so the piano fills whatever rect the host actually grants. A host that clamps the editor gets a correctly sized piano rather than one drawn off the right edge.
- Extra height below the fretboard is black, which is harmless and makes the menu fit.

Two accepted costs, stated so nobody reports them as bugs: the plugin shows a small resize grip in the bottom-right that the desktop does not (theme it from `visuals.widgets.inactive.fg_stroke`, which we own in the build callback), and `Caps::app_sets_size = false` removes the Size submenu from the plugin's menu because size is now a drag, not a percentage.

**Fallback** if the grip or the host resize proves unacceptable: drop `ResizableWindow` for a bare `shell::viewport_ui` and a fixed editor. That is a two-line change and costs only the drag corner.

---

## 6. MIDI

One state machine, two producers, and the audio thread never touches `egui::Context` or allocates.

`MidiEvent` moves to `ivory-ui/src/notes.rs` and gains one variant:

```rust
pub enum MidiEvent { NoteOn { note: u8, velocity: u8 }, NoteOff { note: u8 },
                     Sustain { down: bool }, AllNotesOff }
```

`process_midi_events` (app.rs:269-303) keeps its exact current body, including the `notes_to_release` sustain deferral, and gains one arm:

```rust
MidiEvent::AllNotesOff => {
    self.active_notes.clear();
    self.notes_to_release.clear();
    self.sustain_down = false;
}
```

That is the entire change to the app's MIDI code. **The desktop's `mpsc` inbox is not replaced.** The plugin's editor drains its ring into the same `midi_tx` once per frame, on the GUI thread, so the sustain semantics that spec §10 pins live in exactly one implementation and are not re-expressed in bitfields for the audio thread's convenience.

> This is a deliberate departure from the u128 `NoteState` rewrite. The shipping app gains nothing from it, and `process_midi_events` has zero test coverage today.

### The plugin side

```rust
// plugin/ivory-plugin/src/shared.rs
pub struct NoteBridge {
    /// Exact ordering, preserved. Allocated once in Plugin::default().
    queue:   ArrayQueue<MidiEvent>,     // crossbeam-queue, capacity 8192
    /// Physical key state. The resync source, and the reason a stuck note
    /// cannot outlive one frame.
    held:    [AtomicU64; 2],
    sustain: AtomicBool,
    /// Set on overflow, on reset(), on deactivate(), and on editor spawn.
    resync:  AtomicBool,
}
```

`Plugin::process` is the only audio-thread code:

```rust
const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;   // CC64. Basic has no sustain.

fn process(&mut self, _b, _aux, ctx) -> ProcessStatus {
    while let Some(ev) = ctx.next_event() {
        let m = match ev {
            NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 =>
                MidiEvent::NoteOn { note, velocity: (velocity * 127.0).round().clamp(1.0, 127.0) as u8 },
            NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. }
              | NoteEvent::Choke { note, .. } => MidiEvent::NoteOff { note },
            // 64/127 = 0.5039, so `value >= 0.5` would flip the pedal one
            // step early relative to midi.rs:118's `data2 >= 64`.
            NoteEvent::MidiCC { cc: 64, value, .. } =>
                MidiEvent::Sustain { down: (value * 127.0).round() as u8 >= 64 },
            _ => continue,
        };
        self.notes.record(m);          // one atomic store + one fetch_or/and
        if self.notes.queue.push(m).is_err() { self.notes.resync.store(true, Relaxed); }
    }
    ProcessStatus::Normal              // buffers untouched: bit-identical passthrough
}

fn reset(&mut self)      { self.notes.all_off(); }   // transport stop with keys held
fn deactivate(&mut self) { self.notes.all_off(); }
```

Editor side, once per frame, before `paint`:

```rust
if bridge.resync.swap(false, Acquire) {
    while bridge.queue.pop().is_some() {}
    let _ = tx.send(MidiEvent::AllNotesOff);
    for n in bridge.held_notes() { let _ = tx.send(MidiEvent::NoteOn { note: n, velocity: 100 }); }
    if bridge.sustain.load(Relaxed) { let _ = tx.send(MidiEvent::Sustain { down: true }); }
}
while let Some(ev) = bridge.queue.pop() { let _ = tx.send(ev); }
```

`Editor::spawn` sets `resync = true`. **This is what makes "hold a chord, then open the plugin window" show the chord**, which is the single most natural first gesture and the thing a queue-only design gets wrong.

The audio thread never calls `ctx.request_repaint()`: `nih_plug_egui`'s update closure already calls it unconditionally every frame (`editor.rs`, "For now, just always redraw"). Poking a `Context` from an audio thread takes a lock and can allocate; there is no need and it must never be added.

`MidiConfig::MidiCCs` registers 2080 hidden parameters (`kIsReadOnly | kIsHidden`, `wrapper.rs:558`). Accepted deliberately: `Basic` silently loses the sustain pedal, which would make the plugin behave differently from the app it ships beside. Measure plugin-scan and project-load time in Cubase and Studio One; document it.

`ivory/src/midi.rs` keeps midir, `pick_auto_port` and its tests, and implements:

```rust
// ivory-ui/src/notes.rs
pub trait MidiPorts {
    fn list(&self) -> Vec<String>;
    fn current(&self) -> Option<&str>;
    fn connect(&mut self, name: &str, tx: mpsc::Sender<MidiEvent>, ctx: &egui::Context)
        -> Result<(), String>;
}
```

`IvoryApp` holds `ports: Option<Box<dyn MidiPorts>>`; the plugin passes `None`; `caps.own_midi_ports` and `ports.is_some()` agree.

**The plugin does not link midir.** No CoreMIDI/ALSA/WinMM client inside the DAW's process, no port stealing from the running standalone, no double-feed. If a host refuses to route MIDI to an audio effect (Ableton Live), the answer is a second exported class from the same bundle (`nih_export_vst3!` takes a list), **not** a second MIDI stack.

---

## 7. State with two plugin instances and the standalone all running

| Thing | Behaviour | Why |
|---|---|---|
| **Settings** | The plugin reads `~/.config/ivory/settings.json` **once**, at `Plugin::default()`, as a read-only **seed**. Thereafter its settings live in `#[persist = "settings"] Arc<Mutex<String>>` (the serialised `Settings`), which nih-plug saves with the host project and restores on session recall. The plugin **never writes the file** during ordinary use. | A fresh instance looks like the user's standalone. A reopened session is byte-exact. Two instances on two tracks can hold different capos, tunings and colours. And instance B cannot silently overwrite what instance A chose, which is exactly what a shared last-writer-wins file does today (`Settings::save()` rewrites the whole file, `Settings::load()` runs once per process). |
| **Escape hatch** | One new menu row, built only when `!caps.owns_settings_file`: **"Save Appearance as Default"**. It does a single read-modify-write of the appearance keys only (colours, dark mode, font, wood, tuning, capo, heart), never geometry. | Otherwise the flow is one-way and a user who perfects their colours in a plugin cannot get them back to the app. |
| **Keys meaningless in a plugin** | `window_x/y`, `detached_*`, `fretboard_win_*`, `borderless_mode`, `window_size_percent` are never consulted and round-trip untouched through `#[persist]`, the same way `Settings::extra` already preserves unknown keys. | |
| **Taught chords + learned weights** | **Global and shared**, in `~/.config/ivory/overrides.json`. This is the user's musical data, not per-session appearance. `OverrideStore::save` is already tmp+rename; add `save_merged()` (reload from disk, merge, write) so two writers cannot clobber each other's teaching. | Scoping taught names per-instance would be worse than sharing them. |
| **Single-instance lock** | The plugin **never takes it** (`caps.single_instance = false`). `acquire_single_instance` stays in `ivory/src/main.rs` and is unreachable from `ivory-ui` because `fd-lock` is not a dependency. | The standalone and any number of plugin instances coexist. |
| **Welcome dialog** | Never created in the plugin (guard point 2). | A welcome note on first insert is wrong, and an invisible one deadlocks the piano. |
| **The `#[persist]` caveat** | Hosts mark a project dirty when a **parameter** changes, not when a `#[persist]` field does. A user who recolours and closes without saving loses it. Say so in the release notes; "Save Appearance as Default" covers the case that matters. | |
| **Panics** | `ivory-plugin` installs **no** panic hook (nih-plug installs its own at `wrapper/util.rs`, which logs rather than exits) and wraps the frame in `catch_unwind` into a poisoned flag that paints a static message. `main.rs`'s `process::exit(1)` hook is unreachable because `rfd` and `fd-lock` are not `ivory-ui` deps. | A process-global exit hook inside a DAW takes the host and the user's unsaved session with it. |

`Settings::save_to` becomes atomic (tmp + rename in the same directory) regardless, mirroring `overrides.rs:262-268`. It must keep failing **silently** on a read-only or full config dir and must leave no `.tmp` behind; copy `save_is_atomic_and_leaves_no_temp_file`.

---

## 8. Build, sign, package

### Plugin declaration

```rust
Plugin::NAME    = "Tangent";        // never "Tangent VST"
Plugin::VENDOR  = "ganten";
Plugin::VERSION = ivory_ui::VERSION;
const AUDIO_IO_LAYOUTS: &[AudioIOLayout] = &[stereo_in_out, mono_in_out];
const MIDI_INPUT:  MidiConfig = MidiConfig::MidiCCs;
const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
type SysExMessage = ();  type BackgroundTask = ();
Vst3Plugin::VST3_CLASS_ID = *b"TangentIvoryMon1";   // NEVER CHANGES after release
Vst3Plugin::VST3_SUBCATEGORIES = &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer,
                                   Vst3SubCategory::Tools];
```

A no-audio-bus note effect is technically correct and practically wrong (several hosts, Live among them, will not load one). A stereo passthrough Fx sits anywhere in a chain, and `process()` never touches the buffer so passthrough is free.

`VST3_CLASS_ID` is as permanent as `CFBundleIdentifier`: change it after release and every saved project loses its instance. Write that in HANDOFF next to the bundle-id sentence.

**Parameters: exactly one.** A `BoolParam` with `ParamFlags::BYPASS`. nih-plug does not add one automatically, VST3 hosts expect a bypass on an insert, and it is semantically real (bypass = freeze the display, pass audio). Everything else on the `Params` struct is `#[persist]`: the `EguiState` and the settings JSON. **No parameters for Size / Dark Mode / Tuning / Capo.** One line of reasoning: `#[persist]` and a parameter would be two writers of the same value, and the editor is the settings surface in every host we support.

Fonts: `fonts::install` and `fonts::apply_text_styles` run in `create_egui_editor`'s **build** callback, not once at plugin construction. The editor gets a brand-new `egui::Context` on every open, and because `MenuState::open` measures every row with `ctx.fonts_mut`, getting this wrong makes every menu width and row height silently wrong on the second open rather than visibly broken. Also `queue.bg_color(Rgba::BLACK)` to match `clear_color`.

### `scripts/build-plugin.sh`

1. `export CARGO_TARGET_DIR=~/Library/Caches/ivory-plugin-target`. **Non-negotiable.** The root `target` is already a symlink out of Dropbox because Dropbox breaks build-script linking with "Operation not permitted"; `plugin/target` would be a brand-new Dropbox subdirectory hitting the same wall.
2. Assert: root `cargo metadata` mentions no `nih_plug`; `cargo tree -p ivory-ui` mentions no `eframe`/`midir`/`rfd`; `plugin/Cargo.lock` is clean; the two workspace versions agree; the `egui` version in `./Cargo.lock` equals the one in `plugin/Cargo.lock` (fail otherwise, and escalate to `=0.33.x` pins if it ever fires).
3. macOS: `cargo xtask bundle ivory-plugin --release --target aarch64-apple-darwin` and `--target x86_64-apple-darwin`, then `lipo` into `Contents/MacOS/Tangent`. Hosts still run under Rosetta and an arm64-only plugin is invisible in those.
4. **Rewrite the Info.plist.** `nih_plug_xtask` hardcodes `CFBundleIdentifier = com.nih-plug.{package}` and `CFBundleShortVersionString = 1.0.0`. Write `CFBundleIdentifier = org.codeberg.ganten1998.ivory.plugin` (the app's `org.codeberg.ganten1998.ivory` is untouched; two bundles must not share one identifier, and `.plugin` rather than `.vst3` keeps Steinberg's mark out of every string we choose), `CFBundleName`/`CFBundleDisplayName`/`CFBundleExecutable = Tangent`, `CFBundlePackageType = BNDL`, the numeric core of the workspace version (same truncation `build-macos.sh` does), `LSMinimumSystemVersion = 11.0`, `NSHighResolutionCapable = true`.
5. Install into `Tangent.vst3/Contents/Resources/`: `LICENSE-GPL-3.0`, `LICENSE` (MIT), a plugin-scoped `THIRD-PARTY-LICENSES` generated from `plugin/Cargo.lock`, `OFL.txt` + `font-licenses/` (the OFL obligation is **larger** here: `default_fonts` statically embeds Ubuntu-Light, Noto Emoji, Hack and emoji-icon-font, the same four `build-macos.sh` already covers), and `SOURCE.txt` generated from `plugin/Cargo.lock` naming the **public GitHub** URL (the Codeberg origin is private), the release commit SHA, and the exact `nih_plug`, `nih_plug_egui`/`egui-baseview`, `baseview` and `vst3-sys` revisions.
6. **Verify and exit non-zero** if any of those files is missing from the assembled bundle. LICENSING.md's own words: "If that step is ever dropped, the release is non-compliant, quietly."
7. **Re-sign**, replacing the xtask's ad-hoc signature: `codesign --force --options runtime --timestamp -s "$SIGN_ID"` on `Contents/MacOS/Tangent` then on the bundle, then `codesign --verify --strict --verbose=2`. Reuse `IVORY_SIGN_ID` and the `Developer ID Application` auto-discovery from `build-macos.sh:199`. An ad-hoc-signed `.vst3` is refused by more hosts than an ad-hoc app.
8. Notarize: `ditto -c -k --sequesterRsrc --keepParent`, `xcrun notarytool submit --keychain-profile "$NOTARY_PROFILE" --wait`, `xcrun stapler staple Tangent.vst3`, `stapler validate`.
9. `install` / `uninstall` subcommands that `rm -rf ~/Library/Audio/Plug-Ins/VST3/Tangent.vst3` and then `ditto` a fresh copy. **New inode every iteration.** After an in-place re-sign, syspolicyd serves the cached rejection and the DAW keeps refusing a bundle that is actually fine; `xattr -cr` plus a fresh signature is not enough. This is the same trap this machine already hit with Vesktop and it reads as a code bug for an afternoon.

Windows: `cargo xtask bundle --target x86_64-pc-windows-msvc` (or `cargo xwin`, mirroring `build-cross.sh`), producing `Tangent.vst3\Contents\x86_64-win\Tangent.vst3` (the DLL keeps the `.vst3` extension; that is the format spec). Zip for `C:\Program Files\Common Files\VST3\`. Unsigned, as `tangent.exe` is today.

Linux: build **natively** via the existing remote box, not `cargo zigbuild`; baseview needs X11/xcb headers at link time. `Tangent.vst3/Contents/x86_64-linux/Tangent.so`, installed to `~/.vst3/`. **baseview is X11 only**, so the plugin editor runs through XWayland while the standalone keeps its native Wayland path. Release notes, not a bug.

Delivery: a **separate download** per platform, never fused into the DMG. `scripts/release.sh` gains no plugin step that runs by default. `publish-github.sh` learns two new asset names; the legacy `Ivory-*` alias rule does **not** extend to the plugin, because no plugin link has ever been mailed to anyone.

---

## 9. How the crate graph satisfies LICENSING.md conditions 1-3

**Condition 1, "the plugin stays optional."** Two artifacts, two scripts, two downloads. `scripts/build-macos.sh` produces `Tangent.app`; `scripts/build-plugin.sh` produces `Tangent.vst3`. The installer offers it as a checkbox. Nothing in `ivory` or `ivory-ui` references `ivory-plugin`, and the standalone is fully functional with no `.vst3` anywhere.

**Condition 2, "they never link."** Enforced by cargo, not by care. `plugin/Cargo.toml` carries the empty `[workspace]` table and the root carries `exclude = ["plugin"]`, so `cargo build -p ivory-plugin` from the repo root does not resolve, and `nih_plug`/`vst3-sys`/`baseview` never enter the root `Cargo.lock` and therefore never enter the app's `THIRD-PARTY-LICENSES`. `ivory-ui` is an rlib linked **statically** into two independent executables, which is compile-time sharing, exactly what LICENSING.md permits. No shared dynamic library exists. The plugin does not take the lock file, does not open a socket, does not launch or detect the app, and does not require it. The one thing that could be argued is that both read `~/.config/ivory/settings.json` and `overrides.json`; that is two unrelated programs reading one dotfile, not IPC, and the design has the plugin never write settings.json in ordinary use, which removes even the appearance of coupling. **Add a fourth bullet to LICENSING.md: the plugin must be fully functional with the standalone absent** (missing files already fall back to `Settings::default()` and an empty `OverrideStore`), and never "improve" this into a daemon or a shared dylib without redoing the analysis.

**Condition 3, "each carries its own licence."** `LICENSE` (MIT) beside the app as today; `LICENSE-GPL-3.0` inside `Tangent.vst3/Contents/Resources/`, placed by build-plugin.sh step 5 and asserted by step 6, which exits non-zero rather than warning. Source availability = this repo at the release commit plus the pinned revs, recorded in `SOURCE.txt` **generated from the lockfile** rather than typed by hand, pointing at the public GitHub mirror. Verify the GitHub repo itself is public before the first `.vst3` ships.

**Trademark.** Product name is "Tangent" everywhere a human reads it: `Plugin::NAME`, `CFBundleName`, `CFBundleDisplayName`, the installer checkbox, the release title, the asset name (`Tangent-2.4.0-plugin-macos-universal.zip`, **not** `Tangent-VST3-*`). `Plugin::VENDOR` must not contain "VST" either. The word appears only in the `.vst3` folder extension, in the Windows payload filename, in install paths the OS requires, and in descriptive prose of the form "Tangent plugin (VST3 format)".

Add one row to LICENSING.md's table: `ivory-core`, `ivory-ui`, `ivory`, `ivory-plugin` and every other crate in this repo | MIT | Source, not binaries.

---

## 10. Ordered steps

Every step leaves the desktop app working and `cargo test --workspace` green.

### Step 1 — Tests for the two untested things this plan touches. No production change.

`ivory/src/app.rs` gains a `#[cfg(test)]` module for `process_midi_events`. It has **zero** assertions today. Four behaviours, one test each:
- note-on inserts and cancels a pending release;
- note-off with sustain down moves to `notes_to_release` **only if** the note is currently held;
- note-off with sustain up removes from both sets;
- sustain going down-to-up drains `notes_to_release` out of `active_notes`.

To do this without an `eframe::CreationContext`, add `#[cfg(test)] fn for_tests(settings: Settings) -> Self` that builds the struct with an `egui::Context::default()`. (This constructor disappears in step 3 when `new` takes a `&egui::Context` anyway.)

Also convert `menu.rs`'s `a_submenu_near_the_bottom_is_pulled_back_onto_the_screen` to call the **real** clamp. Extract the clamp out of `show` into `fn clamp_submenu(pos: Pos2, size: Vec2, menu_x: f32, bounds: Rect) -> Pos2` and have both `show` and the test call it. The test currently reimplements it as a local closure and exercises zero lines of `menu.rs`; this is the code the next two steps move.

Acceptance: `cargo test --workspace` goes from 213 to 218; `cargo build --workspace --all-targets` stays warning-free; no non-test line of behaviour changed.

### Step 2 — Atomic `Settings::save_to`. Ships on its own.

`settings.rs:434-442` becomes tmp + rename in the same directory, mirroring `ivory-core/src/overrides.rs:262-268`:

```rust
fn save_to(&self, path: &std::path::Path) {
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    let Ok(text) = serde_json::to_string_pretty(&Value::Object(self.to_map())) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        if std::fs::rename(&tmp, path).is_err() { let _ = std::fs::remove_file(&tmp); }
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}
```

This is a desktop bug fix today: 22 call sites in `app.rs` truncate the file in place, and `load_from` treats any parse error as "return every default", so a crash or a concurrent read mid-write costs the user every colour they ever picked. It becomes routine the moment a second process exists.

New tests: a truncated file is not mistaken for valid empty settings; no `.tmp` is left behind (copy `save_is_atomic_and_leaves_no_temp_file`); the write still fails silently on a read-only directory. `unknown_keys_preserved_and_key_order_stable`, `garbage_file_yields_defaults`, `window_geometry_round_trips_including_negative_coordinates`, `half_written_position_is_ignored_rather_than_half_applied` and `wrong_typed_keys_fall_back_per_key` must stay green untouched. `~/.config/ivory` is not on Dropbox, so there is no dataless-file hazard.

### Step 3 — Carve out `ivory-ui`. Pure move, nothing else in the commit.

```bash
mkdir -p ivory-ui/src ivory-ui/tests
git mv ivory/src/{app,piano,chord_strip,fretboard_panel,menu,dialogs,fonts,settings,shell}.rs ivory-ui/src/
git mv ivory/tests/license_feature.rs ivory-ui/tests/
```

- New `ivory-ui/src/lib.rs`: nine `pub mod` lines, `pub mod notes;`, and `pub const VERSION`.
- New `ivory-ui/src/notes.rs`: `MidiEvent` and `parse_message` cut from `ivory/src/midi.rs` **with their tests**, plus the `MidiPorts` trait (unimplemented for now).
- New `ivory-ui/Cargo.toml` with `version.workspace = true`, `license.workspace = true`, the `ivory-core = { features = ["learning", "license"] }` line **moved** from `ivory/Cargo.toml:29`, `egui`, `serde`, `serde_json`, `dirs`, `log`, and `[dev-dependencies] ttf-parser = "0.25"` (moved; `fonts::embedded_fonts_cover_delta_and_oslash` needs it, and a missing dev-dep fails loudly rather than silently).
- New `ivory/src/desktop_shell.rs`: `DesktopApp { app, shell }` + `impl eframe::App` (the block moved from `app.rs:1118-1136`), because the orphan rule forbids implementing a foreign trait on a foreign type.
- `ivory/src/main.rs`: drop the nine `mod` lines, `use ivory_ui::{app::IvoryApp, settings::Settings};`, keep `#![windows_subsystem]`, the CLI, the logger, the panic hook, the lock, the icon, `run_native`.
- Root `Cargo.toml`: add `ivory-ui` to `members`, add `exclude = ["plugin"]` now so it never gets forgotten.
- `scripts/gen-third-party-licenses.sh`: `skip = {"ivory", "ivory-core", "ivory-ui"}`. Regenerate `THIRD-PARTY-LICENSES` and check the diff is empty apart from crate count.
- HANDOFF §3 (repo layout) and §5 (commands) updated **in this commit**, not afterwards.
- Copy `license_feature.rs` back into `ivory/tests/` as well. One file, and it catches a future direct dep.

Acceptance, and this is the tripwire that matters: **`cargo test --workspace` must report the identical count**, 218, with the 48 unit tests appearing under the `ivory-ui` target instead of the `ivory` target. Record the exact before/after per-target counts in the commit message. A module accidentally omitted from `lib.rs` makes its tests vanish while the suite stays green. Also add and run the four firewall greps from §1. `cargo run --bin tangent` must be indistinguishable; take a `screencapture -x -o` before and after and decode both.

### Steps 4 onward

4. **`Shell` / `Caps` / `SurfaceSpec` / `SurfaceReport` / `SurfaceOut` in `ivory-ui/src/shell.rs`, plus `ViewportShell` in `ivory/src/desktop_shell.rs`.** Route all five `show_viewport_immediate` sites and all nine `send_viewport_cmd` sites through it; add the eleven guard points with `Caps::DESKTOP`; add the `Caps::DESKTOP` all-true / `Caps::PLUGIN` all-false test and the `assert_send::<IvoryApp>()` static assertion. Desktop behaviour unchanged. **Review this commit harder than any other**, field by field against the old `ViewportBuilder`s: the submenu's `with_active(false)` (lose it and Escape-to-close breaks in a way nobody notices until a tester reports it), dialogs' `with_always_on_top`, `with_decorations(!borderless)`, and the `last_sent_size` / `decorations_sent` latches (lose either and the app sends three viewport commands every frame forever). Add a counting fake `Shell` asserting one triple per change and zero per steady frame. `a_tiling_wm_is_detected_but_ordinary_jitter_is_not` does not move and does not change; the Caps guard goes **around** the call, never inside the predicate.

5. **`monitor: Option<Vec2>` → `bounds: Rect` everywhere placement is clamped**: `menu.rs:471`, the extracted `clamp_submenu`, `dialogs::Placement`, `settings::clamp_to_monitor` → `clamp_to_bounds`, `app.rs`'s `monitor_size`. Desktop passes `Rect(0,0..monitor)` so nothing moves; `Rect::EVERYTHING` preserves the "unknown" semantics. Clamp to `bounds.min`, not to 0.0. Step 1's real-clamp test and `a_dialog_centres_on_the_window_or_declines_to_place_itself` must both stay green.

6. **`fit_or_scroll` + `InlineShell` + `HeadlessShell` + `IVORY_INLINE=1` / `IVORY_CANVAS=WxH`.** Add the drawn title bar to `show_dialog_viewport`'s inline path. Add the both-shells surface-invariant test (every `SurfaceSpec` through `ViewportShell` and `InlineShell` at 2560x1440 and 1300x200; assert `max_rect` equals the pane rect, every rect is inside `bounds`, every interactive row is at least `row_h` tall). **Then do the taste pass**: run the desktop under `IVORY_INLINE=1` in light and dark mode, screenshot, decode to BMP, count pixel runs (52 key runs, 51 separators, 0 background runs), and look at the scrolled menu and every dialog. This is where the plugin's UI is judged, in the shipping binary, before nih-plug exists.

7. **`Caps`-gate the menu.** `build_entries(view, caps)` drops Size, Borderless, Select MIDI Input and the two Detach/Attach pairs; adds "Save Appearance as Default". Extend the existing `rows(v)` / `submenus(v)` helpers with a `Caps::PLUGIN` case asserting no surviving row's action needs a window, and that under `Caps::DESKTOP` the row set is byte-identical.

8. **`band_sizes_for_width` + `MidiEvent::AllNotesOff` + `MidiPorts`.** The additive layout branch, the one new `process_midi_events` arm, and moving the midir connect out of `IvoryApp::new` into `main.rs` behind the trait. `size_math_matches_python_int_truncation` and `the_fretboard_band_joins_the_stack_at_every_size` stay green **and unedited**.

9. **`OverrideStore::save_merged`** in `ivory-core` (reload, merge, write, still tmp+rename), used by the teach and correct paths. One test: two stores teaching different chords both survive.

10. **Create `plugin/`. `Plugin` impl only, no editor.** The quarantined workspace with the ivory-keygen comment, `bundler.toml`, `xtask`, `.cargo/config.toml` with the `xtask` alias, the `CARGO_TARGET_DIR` symlink created now rather than discovered later. `NoteBridge`, `process()`, `reset()`, `deactivate()`, the Bypass param, the two `#[persist]` fields, `nih_export_vst3!`. Mirror `license_feature.rs` into `plugin/ivory-plugin/tests/`. **Milestone: `cargo xtask bundle ivory-plugin --release` produces `Tangent.vst3`, a validator and Reaper both scan and load it, audio passes through unchanged, the parameter list is sane, and `nih_log!` shows note events arriving.**

11. **Wire the editor.** `create_egui_editor` with `fonts::install` + `apply_text_styles` in the **build** callback; `ResizableWindow::show` in the update callback; drain the bridge into `midi_tx`; `IvoryApp::paint(ui, &mut PluginShell)`; `catch_unwind`; `keyboard_wanted` → `Queue::set_key_capture`; `queue.bg_color(BLACK)`; settings seeded from `#[persist]` if present, else from the file, else defaults. **This is v1's payload.**

12. **`scripts/build-plugin.sh`** exactly as §8. Then `publish-github.sh`'s two new asset names.

13. **Host matrix, by hand, before any polish.** Reaper, Bitwig, Cubase, Studio One, Ableton Live, FL Studio. For each: does it load; does MIDI reach it; **type into the Supporter Key and Teach Chord Name fields**; open/close/reopen the editor five times (fonts must survive; held notes must reappear); save and reload a session; two instances in one project with different colours and capos; transport stop with keys held; drag the editor corner; does the spacebar still start the transport.

14. **Docs, one commit.** LICENSING.md gains the fourth bullet, the rlib rule and the table row; HANDOFF gains §2e and updated §3/§5; DIVERGENCES gains the plugin's deviations; RELEASE.md gains per-host placement and install paths; README says the plugin is optional, GPLv3, VST3 format, and that Logic and Pro Tools are out.

15. **Only after 13**, decide whether a second exported class is needed for hosts that will not route MIDI to an audio effect. `nih_export_vst3!` takes a list, so it is one more type in the same binary with `Vst3SubCategory::Instrument`, no audio input, a silent stereo output, and its own permanent class ID.

---

## 11. Explicitly NOT in v1

- **Column reflow of the menu.** `fit_or_scroll` covers it; the editor is resizable so the user can make it fit. Reflow is new UI with desktop-unreachable break-point logic.
- **Any change to the eleven dialog bodies.** No `Fit::Wide` two-column arrangements, no hand-rolled horizontal colour picker. Every body scrolls unchanged.
- **Detached windows in the plugin.** `Caps::detachable = false` removes the rows rather than faking them (HANDOFF §2c).
- **A MIDI device picker in the plugin.** No midir in the DAW's process, ever.
- **VST3 parameters for settings.** One Bypass, and nothing else.
- **A second exported Instrument class.** Step 15, after measurement.
- **AU, CLAP, AAX, standalone-via-nih-plug.** VST3 only, one deliverable.
- **The desktop switching to `InlineShell` by default.** `IVORY_INLINE=1` is a developer switch. `ViewportShell` is and stays the desktop.
- **Any change to `ivory-core`'s engine.** The chord engine, the voicing solver, the 13,133-row differential, `acceptance.rs`, `voicing_acceptance.rs` and `voicing_stress.rs` see no diff. The only `ivory-core` change in the whole plan is `save_merged`.

---

## 12. Traps, each with its specific prevention

| Trap | Prevention |
|---|---|
| Tests silently disappearing when nine modules change crate | Step 3 is a pure move whose commit message records the exact before/after per-target counts. A count that drops is a missing `pub mod`. |
| The `license` feature line left behind in `ivory/Cargo.toml` | It moves with the dependency, the test moves with it, a copy stays in `ivory/tests/`, and `plugin/ivory-plugin/tests/` gets its own. Otherwise every supporter key is rejected inside the DAW while all 218 tests stay green. This has bitten once already. |
| A dropped `ViewportBuilder` field in step 4 | `SurfaceSpec`'s fields are exactly the union of the five existing builders, so a dropped field is a missing struct field. Plus the recording fake asserting `with_active(false)` on the submenu and `always_on_top` on dialogs. |
| The geometry latches turning into per-frame commands | `last_sent_size` and `decorations_sent` move **into** `ViewportShell`; a counting fake asserts one triple per change, zero per steady frame. |
| `clamp_to_bounds` snapping negative-coordinate multi-monitor setups to the primary origin | Clamp to `bounds.min`, not 0.0; `Rect::EVERYTHING` for unknown; the existing round-trip test stays green. |
| The `nih_plug` rev drifting from `28b149ec` | Pinned in one place with a comment saying why; `plugin/Cargo.lock` committed; the error otherwise is "expected Editor, found Editor". |
| `nih_plug_egui` taking `nih_plug` with `default-features = false`, dropping `vst3` | Our manifest sets `features = ["vst3"]` explicitly; without it `nih_export_vst3!` does not exist. |
| `11f487f` being rebased away | Both revs are already in `~/.cargo/git/db` and in `plugin/Cargo.lock`. Mirror both repos to Codeberg as pinned read-only forks the moment the first release ships. Last resort is vendoring `editor.rs` + `lib.rs` + `resizable_window.rs` (about 400 lines, MIT, attribution retained). |
| Two lockfiles drifting to different `0.33.x` patch releases | The version tripwire in build-plugin.sh; escalate to `=0.33.x` if it fires. |
| `plugin/target` inside Dropbox reproducing "Operation not permitted" | `CARGO_TARGET_DIR` symlink created in step 10, not discovered in step 12. |
| A panic unwinding through baseview's objc/Win32 FFI (UB, takes the DAW) | No panic hook in the plugin, `catch_unwind` around the frame into a poisoned flag, and the CI grep that `ivory-ui` and `ivory-core` contain no `process::exit` and no `rfd`. `voicing_stress.rs` exists because a real overflow panic was found in the solver; that panic is now a DAW crash. |
| macOS library validation | A DAW running hardened **without** `com.apple.security.cs.disable-library-validation` refuses a plugin not signed by its own team. Most carry it. Developer ID + hardened runtime + notarize + staple, or expect "it doesn't show up in my plugin list" reports indistinguishable from a scan failure. |
| The re-sign cache trap on Tahoe | `install`/`uninstall` subcommands that `rm -rf` then `ditto`. New inode every iteration. |
| `IVORY_DEMO_NOTES` read every frame | `app.rs:312` calls `std::env::var` inside `display_notes()`: a process-global lock plus an allocation, per instance, sixty times a second. Read it once at construction. One line, do it in step 8. |
| "Just save it, it's one line" reopening settings.json writes from the plugin | `Shell::persist` is the **only** path to disk from `ivory-ui`; do not re-expose `Settings::save` there. |
| Scope creep back into column reflow or dialog rewrites | The arithmetic is written down here: a 545pt menu in a 200pt editor is not moved UI, it is new UI, and the editor is resizable. |

---

## 13. What the desktop keeps doing, literally

`main.rs` keeps its CLI, `-l`/`-p`, the stderr logger, the panic hook with the rfd box, the `fd-lock` single instance, the icon loading, the `ViewportBuilder` with the fixed-size triple, and `eframe::run_native`. All five child-window sites still create real OS viewports with the same ids at the same monitor positions. midir still auto-connects on the same priority chain. Settings still write on every mutation (now atomically). The menu, all eleven dialogs, both popouts, D-UI-10 through D-UI-17, the tiling-WM guard, the geometry debounce and the offscreen rescue all behave exactly as they do today, because `Caps::DESKTOP` is all-true and every guard is `if true`.

The desktop gains exactly two things: settings writes that cannot be torn, and a line in About and the README saying a plugin exists. Everything else is invisible, and that is the measure of success for steps 1 through 8.
---

## Addendum, 2026-08-13: the dependency question, settled by building it

The plan's §0 is right and the earlier working assumption was wrong. Verified
here by compiling, not by reading:

```toml
[dependencies]
nih_plug      = { git = "https://github.com/robbert-vdh/nih-plug.git",
                  rev = "28b149ec4d62757d0b448809148a0c3ca6e09a95",
                  features = ["vst3"] }
nih_plug_egui = { git = "https://github.com/BillyDM/egui-baseview.git" }
egui          = "0.33"
```

`cargo check` is clean and the lockfile holds exactly ONE copy of each of
`egui 0.33.3`, `egui-baseview 0.7.0`, `nih_plug`, `nih_plug_egui 0.1.0` and
`baseview`. No `[patch]`, no fork, no vendoring, because `nih_plug_egui` 0.1.0
now lives INSIDE the egui-baseview repo and already sits on egui 0.33.

For the record, the road not taken: depending on `nih_plug_egui` from the
nih-plug repo DOES need patching, and patching it is worse than it looks.
`nih_plug_egui` (nih-plug's copy) pins baseview `9a0b42c` while egui-baseview
0.7 pins `237d323`, so patching only `egui-baseview` resolves and then fails to
compile with "expected `baseview::WindowHandle`, found
`baseview::window::WindowHandle`" — two revs of one crate are two types. Both
have to move together, and cargo rejects a patch pointing at the same URL, so it
would need two forks. All of that disappears by taking `nih_plug_egui` from the
repo it actually lives in. The `rev` on nih_plug is still mandatory: it must be
the one egui-baseview's workspace pins, or there are two `nih_plug` packages and
`Editor` stops being `Editor`.

---

## Addendum: the installer is required for the NEXT release

Owner's requirement, 2026-08-13: an installer for all three platforms offering
**standalone only / plugin only / both**, placing each in the correct system
location. This moves the installer out of "later" and into the next release, and
it changes two things in the plan above.

**It is no longer optional polish, it is the delivery mechanism.** Today every
platform ships an archive the user unpacks by hand, and LICENSING.md condition 1
("the plugin stays optional") is currently satisfied by the plugin simply not
existing. Once there is an installer, that condition becomes a real UI element:
the plugin checkbox must default to OFF or be an explicit choice, never a silent
inclusion, and declining it must leave nothing GPL-licensed on disk.

**Destinations, which are not negotiable per platform:**

| Platform | Standalone | VST3 |
|---|---|---|
| macOS | `/Applications/Tangent.app` | `/Library/Audio/Plug-Ins/VST3/Tangent.vst3` (system) or `~/Library/Audio/Plug-Ins/VST3/` (user, no admin) |
| Windows | `%ProgramFiles%\Tangent\` | `%CommonProgramFiles%\VST3\Tangent.vst3` |
| Linux | `/usr/local/bin/tangent` + `.desktop` in `/usr/local/share/applications` | `~/.vst3/Tangent.vst3` or `/usr/local/lib/vst3/` |

The VST3 paths are defined by the format's own location spec and hosts scan
exactly those; anywhere else and the plugin does not appear.

**Per-platform tooling, cheapest path first:**
- **macOS**: `productbuild` with a distribution XML giving two selectable
  choices. Signs with the same Developer ID (an installer needs a *Developer ID
  Installer* certificate, which is a SEPARATE cert from the Application one
  already in use) and notarizes as a `.pkg`. Prefer the user-domain VST3 path so
  no admin prompt is needed.
- **Windows**: Inno Setup or WiX. Unsigned today, so SmartScreen warns on the
  installer as well as the app; this makes the Azure Trusted Signing decision
  more pressing, because an unsigned *installer* reads worse than an unsigned
  binary.
- **Linux**: a shell installer in the tarball is the honest 80% (`install.sh
  --standalone --vst3 --prefix`). Native packaging (.deb/.rpm/AUR/xbps) is a
  per-distro treadmill and should wait for demand.

**Uninstall matters as much as install**, especially on macOS where a stale
`.vst3` in a scanned directory means a DAW keeps loading an old build. Ship an
uninstaller or a documented list of the exact paths touched.

**Sequencing against the plan above:** the installer cannot be built before the
plugin exists, so it lands after step 11 (`scripts/build-plugin.sh`) as step 12.
But the DESTINATIONS should be decided now, because the bundle identifier, the
`.vst3` bundle name and the CID are all frozen forever on first release and the
installer is what makes them visible.


---

## Progress log

Steps completed, with what each actually taught. Kept here rather than in the
commit log because the next session reads this file first.

**1-2. Freeze the seams (`c983678`).** The note/sustain state machine had ZERO
coverage while being what every display reads from; extracted to `NoteState` and
given six tests including a 2,000-stream property check. `Settings::save_to` was
a bare `fs::write`, and `load_from` answers a parse error with all-defaults which
it then saves over the wreckage — one torn write costs every setting the user
ever chose. Now write-then-rename. Rare with one writer; routine once several
plugin instances share the file.

**3. `ivory-ui` extracted (`441bcfd`).** Nine modules moved by `git mv`,
unchanged. The set was already closed: they reference each other and `ivory-core`
and nothing else, and not one mentioned eframe, midir, rfd or fd-lock.
**229 tests before, 229 after** — `ivory-ui` 53, the binary 11. `app.rs` stays in
the binary for now because it still owns eframe.

`scripts/check-firewall.sh` is the enforcement, and it was verified to FAIL:
planting a `process::exit` in `ivory-ui` makes it exit non-zero and name the
line. Two things it caught in passing:
`gen-third-party-licenses.sh` had a hardcoded skip list, so `ivory-ui` would have
shipped in the MIT app's `THIRD-PARTY-LICENSES` as a third-party dependency of
itself; and `dialogs.rs`'s `env!("CARGO_PKG_VERSION")` now reads `ivory-ui`'s
version, so `ivory_ui::VERSION` names that coupling.

**4a. `Caps` (`host.rs`), and the menu consumes it.** Plain data, not a trait:
every field is a fact the UI needs at a branch point, and a struct of bools can
be built in a test for a host that does not exist yet. Fields name a CAPABILITY
and never a host — `if caps.child_windows` still reads correctly the day someone
writes a CLAP build; `if is_plugin` would need revisiting.

Under `Caps::PLUGIN` the menu drops Size, Borderless, Select MIDI Input and both
Detach pairs, and keeps everything that is pure state: dark mode, keytoggle,
teach, correct, learning, guitar view, Wood, Tuning, Capo, colours, About,
supporter key. Two tests hold the line — one asserts the DESKTOP menu is
unchanged, one asserts no surviving PLUGIN row needs a window or a device.

### Next: the surface seam (step 4b)

The remaining five `show_viewport_immediate` sites, all of which funnel through
`shell::viewport_ui`. **Read §0 before starting**: those calls do not fail in a
plugin, they run EMBEDDED and open a second `CentralPanel` under an identical id,
painting garbage over the piano. The seam therefore has to sit ABOVE
`viewport_ui`, not at it. `Caps::child_windows` is the flag that already exists
to decide it; `keys.rs`'s help card is the shape the in-canvas version takes,
and it is already written and shipping.

---

## Progress log, 2026-08-14 (later): steps 5-8 are DONE

The plugin builds, loads, instantiates, and is packaged into installers on
macOS and Windows. See `docs/HANDOFF.md` §2f for the state and what is left.

**Step 5 was smaller than this plan assumed, and the reason matters.**
`send_viewport_cmd` is `egui`, not `eframe`, so no `Shell` trait was needed to
route it — `app.rs` moved into `ivory-ui` keeping every call, gated on `caps`.
The `Shell`/`SurfaceSpec` design in §2 was therefore NOT implemented as
written. What replaced it is smaller and is already proven by the 4b work:
`shell::surface(ctx, caps, spec, add)`, one function that draws a pane either
as an OS viewport or as an in-canvas `Area`, used by the menu, the submenu and
all eleven dialogs. `Caps` stayed at five fields, not eight.

The real seam was three things:
  * `eframe::CreationContext` -> `&egui::Context` (nothing else was used),
  * `midir` -> the `ports::MidiPorts` trait, with `MidiEvent` and
    `parse_message` moved to `ivory-ui`; the mpsc channel is deliberately NOT
    behind the trait, because a plugin fills the same one from `process()`,
  * `impl eframe::App` -> `ivory/src/desktop.rs`, which the orphan rule forces
    and which is the compiler stating that eframe is the binary's business.

**Step 6-7** landed as described: quarantined workspace, `nih_export_vst3!`,
`#[persist]` state. The dependency recipe in the 2026-08-13 addendum was
correct and needed no adjustment — it compiled first try, one copy of every
crate, zero GPL crates in the root lock.

**Step 8** produced `scripts/build-plugin.sh` (bundle layout taken verbatim
from `nih_plug_xtask`, because its own bundler hardcodes
`com.nih-plug.<package>` and version 1.0.0) and `scripts/build-installer.sh`
plus `installer/{macos,windows,linux}/`.

### Corrections to this document

* §2's `Shell` trait, `SurfaceSpec`, `SurfaceReport`, `InlineShell`,
  `ViewportShell` and `HeadlessShell` were **not built**. Read
  `ivory-ui/src/shell.rs` instead; it is 300 lines including tests.
* §6's MIDI section describes `self.active_notes` / `self.notes_to_release`,
  which the `NoteState` extraction replaced before this round began.
* The `moduleinfo.json` question is settled: nih-plug never writes one (zero
  hits across the checkout) and no host required it.
* `cargo xtask bundle` is not used, so §8's `chdir_workspace_root` trap — real,
  and it would have fired the moment `plugin/` landed inside the repo — is
  moot rather than worked around.
