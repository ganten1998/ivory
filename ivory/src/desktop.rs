//! The desktop half of the app: an eframe window, and a real MIDI device.
//!
//! Everything that draws lives in `ivory_ui::app::IvoryApp`, which the VST3
//! build also uses. What is left here is exactly the three things a standalone
//! has and a plugin editor does not — an `eframe::App` impl, a `midir`
//! connection, and a command line — plus the `Caps` value that says so.
//!
//! The wrapper struct is not ceremony. `impl eframe::App for IvoryApp` cannot
//! be written from this crate: the orphan rule forbids implementing a foreign
//! trait for a foreign type. That is a feature here rather than an obstacle —
//! it is the compiler stating that eframe is this crate's business and not the
//! shared crate's.

use crate::midi;
use ivory_ui::app::IvoryApp;
use ivory_ui::host::Caps;
use ivory_ui::midi_event::MidiEvent;
use ivory_ui::ports::MidiPorts;
use ivory_ui::settings::Settings;
use std::sync::{mpsc, Arc};
// Only the recorder's camera-permission latch uses it, and a Minimal build has
// no recorder — so an ungated import is an unused-import warning there.
#[cfg(feature = "recorder")]
use std::sync::Mutex;

/// A real MIDI input, opened with `midir`.
///
/// Holds the egui context so the callback thread can wake the UI on every
/// event: repaints are event-driven rather than busy-looped (D-UI-3). A plugin
/// needs no equivalent — the host calls `process()` and then the editor.
pub struct DeviceMidi {
    ctx: egui::Context,
    conn: Option<midi::MidiConnection>,
    /// The recorder's raw feed. Owned here rather than by the recorder, and
    /// **created once per app** rather than once per connection, because midir
    /// seals a callback's captured state the moment the port opens — see
    /// `midi::RawMidiTap`. Every connection gets a clone of this `Arc`, so
    /// switching ports keeps the history the tick-0 controller snapshot needs.
    tap: Arc<midi::RawMidiTap>,
    /// The app's single time origin. See [`DeviceMidi::timebase`].
    #[cfg(feature = "recorder")]
    timebase: ivory_record::audio::Timebase,
}

impl DeviceMidi {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            ctx,
            conn: None,
            // ~10 minutes of dense playing before anything is shed, which is
            // far more than the pre-roll needs and cheap: a few hundred KB.
            tap: Arc::new(midi::RawMidiTap::new(60_000)),
            #[cfg(feature = "recorder")]
            timebase: ivory_record::audio::Timebase::new(),
        }
    }

    /// The startup priority chain (spec §10). Silent on failure by design: the
    /// app runs without MIDI rather than opening a dialog nobody asked for.
    pub fn auto_connect(&mut self, tx: mpsc::Sender<MidiEvent>) {
        self.conn = midi::auto_connect(tx, self.ctx.clone(), Arc::clone(&self.tap));
    }

    /// The raw feed, for the recorder.
    #[cfg(feature = "recorder")]
    pub fn tap(&self) -> Arc<midi::RawMidiTap> {
        Arc::clone(&self.tap)
    }

    /// The one epoch every stamp in the app is measured against.
    ///
    /// Owned here rather than by the session because the MIDI tap starts
    /// stamping the moment a port opens, which is before any recorder exists.
    /// One `Timebase`, created once, shared: two of them would put the MIDI and
    /// the audio in different worlds and every take would carry a constant
    /// offset nobody could account for.
    #[cfg(feature = "recorder")]
    pub fn timebase(&self) -> ivory_record::audio::Timebase {
        self.timebase
    }
}

impl MidiPorts for DeviceMidi {
    fn list(&self) -> Vec<String> {
        midi::list_port_names()
    }

    fn connect(&mut self, name: &str, tx: mpsc::Sender<MidiEvent>) -> Result<(), String> {
        // Close the old port FIRST (parity): some drivers refuse a second open
        // of the same device, so holding both across the switch fails on the
        // machines that matter and works on the ones that do not.
        self.conn = None;
        self.conn = Some(midi::connect_by_name(
            name,
            tx,
            self.ctx.clone(),
            Arc::clone(&self.tap),
        )?);
        Ok(())
    }

    fn current(&self) -> Option<String> {
        self.conn.as_ref().map(|c| c.port_name.clone())
    }
}

/// The recorder, and everything that has to happen around a frame to drive it.
///
/// Absent from a Minimal build, where `ivory-record` is not linked at all.
#[cfg(feature = "recorder")]
struct Recorder {
    session: crate::record::Session,
    /// The monitor output: the hosted instrument and the click, summed in one
    /// callback.
    ///
    /// Its life is tied to the BAND rather than to the app, for the same reason
    /// the input stream's is: an output device held open by a chord display
    /// nobody is recording with is a device another app cannot get exclusive
    /// access to. `None` when the band is closed, or when the device would not
    /// open at all.
    engine: Option<crate::instrument::Engine>,
    /// Why the output device would not open, if it would not.
    engine_error: Option<String>,
    /// What the engine has in each slot, so a change in settings is noticed on
    /// the edge rather than re-decided every frame.
    plugin_loaded: [Option<String>; ivory_ui::recorder::SLOTS],
    /// The slot whose load has been announced but not yet performed.
    ///
    /// `load_plugin` blocks for **about five seconds** — the module's own
    /// initialiser, then a warm-up, because four of six instruments on this
    /// machine render silence if recorded cold. That happens on the UI thread,
    /// so doing it the moment the selection changes freezes the window for five
    /// seconds with the previous frame still painted and nothing on screen
    /// saying why. Same two-phase treatment the camera already gets: announce
    /// on one frame, block on the next.
    plugin_opening: Option<usize>,
    audio: crate::devices::Shared,
    camera: crate::devices::Shared,
    /// Why enumeration failed last time, so the band can say "permission" and
    /// not "no cameras" — two problems with completely different fixes.
    camera_denied: Arc<Mutex<Option<String>>>,
    /// A camera open that has been announced but not yet performed.
    ///
    /// `open_camera` blocks the calling thread for 63 ms on a built-in camera
    /// and a measured 1.9-3.9 s on an external UVC one, and it runs on the UI
    /// thread. Doing it the moment the selection goes stale freezes the window
    /// for up to four seconds with the PREVIOUS frame still painted and nothing
    /// saying why. So the intent is recorded on one frame — which paints
    /// "starting the camera…" and asks for an immediate repaint — and the
    /// blocking call happens on the next.
    camera_opening: bool,
    /// When the camera was first noticed to be running-but-silent, so the
    /// warning waits a few seconds rather than firing on frame one.
    camera_silent_since: Option<std::time::Instant>,
    /// The uploaded preview frame.
    ///
    /// Kept between frames on purpose. A 30 fps camera in a 60 fps window
    /// delivers nothing on half the frames, and a preview that cleared itself
    /// on a `None` would strobe black at 30 Hz.
    preview: Option<egui::TextureHandle>,
    preview_px: egui::Vec2,
    /// Whether the band was open on the previous frame, so opening and closing
    /// the input happens on the EDGE rather than being re-decided sixty times a
    /// second.
    band_was_open: bool,
    /// Recomputed on a timer rather than every frame: `statvfs` is a syscall,
    /// and the answer changes by megabytes, not by pixels.
    disk_checked_at: Option<std::time::Instant>,
    disk_bytes: Option<u64>,
    /// The take's video, while one is being filmed.
    #[cfg(target_os = "macos")]
    video: Option<TakeVideo>,
    /// Whether this take has already tried to start filming.
    ///
    /// Without it a take whose video was REFUSED — no camera, no GPU, a file
    /// that would not open — retries on every window frame, and rewrites the
    /// same error sixty times a second over whatever else the band was saying.
    ///
    /// Only read on macOS, because that is the only platform with an encoder.
    /// Kept unconditionally so the field list does not fork.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    video_tried: bool,
    /// The newest camera frame, kept as RGBA for the compositor.
    ///
    /// A copy, and a deliberate one: the preview uploads its own texture and
    /// then drops the frame, but a video tick happens on the take's clock and
    /// not the window's, so the pixels have to still be here when it does.
    /// Without this the video would only ever contain frames that happened to
    /// land on the same window frame as a tick.
    camera_rgba: Option<(Vec<u8>, u32, u32)>,
    /// When to try starting the monitor output again, and how many tries are
    /// left.
    ///
    /// `start_engine` runs on the edge of the band opening, which is fine while
    /// the only way to lose the engine is to close the band. Changing the
    /// buffer size drops it deliberately and reopens the SAME CoreAudio device
    /// in the same breath — the one moment a transient failure is likely — and
    /// without a retry the app would sit there with no monitor and no
    /// instrument until somebody thought to close the band and open it again.
    ///
    /// Bounded, because a device that is genuinely gone must not be reopened
    /// sixty times a second for the rest of the session. The band shows the
    /// error either way.
    engine_retry: Option<(std::time::Instant, u8)>,
    /// The count-in downbeat the session has already been armed with.
    ///
    /// See the use site: `count_in_done` is a latch, so without this the same
    /// instant is handed over every frame for the rest of the session.
    armed_downbeat: Option<i64>,
    /// The buffer size both streams were opened with.
    ///
    /// Changing it has to REOPEN them — a running stream cannot be resized —
    /// and reopening the output means reloading every instrument, which is
    /// five seconds each. So it is done on the edge, and never while a take is
    /// rolling: a take whose buffer changed halfway through is a take with a
    /// hole in it.
    buffer_open: Option<u32>,
    /// The last finished take this host has already accounted for.
    ///
    /// Updated whether or not "Show when done" is ticked, which is the point:
    /// without it, ticking the box after a take would immediately open a Finder
    /// window for a recording made ten minutes ago.
    seen_take: Option<String>,
    /// `IVORY_OPEN_EDITOR=1` bookkeeping. See `after_frame`.
    dev_editor_at: Option<std::time::Instant>,
    dev_editor_done: bool,
}

/// How long `IVORY_OPEN_EDITOR` waits before opening, so the plugin has
/// finished loading and the window has drawn at least one ordinary frame.
#[cfg(feature = "recorder")]
const DEV_EDITOR_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// The standalone app: `IvoryApp`, plus the eframe trait impl it cannot carry.
pub struct DesktopApp {
    app: IvoryApp,
    #[cfg(feature = "recorder")]
    recorder: Recorder,
    /// The launch splash, until it has been earned and faded. `None` after.
    splash: Option<Splash>,
}

#[cfg(feature = "recorder")]
const DISK_RECHECK: std::time::Duration = std::time::Duration::from_secs(5);

/// How long to wait before trying the monitor output again, and how many times.
///
/// Half a second is long enough for CoreAudio to finish releasing a device that
/// was dropped a moment ago, and five tries is long enough to cover it without
/// becoming a device that is reopened for ever.
#[cfg(feature = "recorder")]
const ENGINE_RETRY_AFTER: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(feature = "recorder")]
const ENGINE_TRIES: u8 = 5;

#[cfg(feature = "recorder")]
impl DesktopApp {
    /// Everything the band shows, refreshed from what is actually true.
    ///
    /// Pushed IN rather than pulled out, because `ivory-ui` cannot reach a
    /// device or a filesystem and must not learn how.
    fn fill_recorder_state(&mut self, ctx: &egui::Context) {
        use ivory_record::take;

        let root = self.app.record_root();
        let spec = self.app.export_spec();
        let name = self.app.take_name().map(str::to_owned);

        // Free space, on a timer.
        let now = std::time::Instant::now();
        if self
            .recorder
            .disk_checked_at
            .is_none_or(|t| now.duration_since(t) >= DISK_RECHECK)
        {
            self.recorder.disk_checked_at = Some(now);
            self.recorder.disk_bytes = crate::record::available_bytes(&root);
        }

        // The camera. Uploaded here rather than in `after_frame` because the
        // texture has to exist before the band that draws it is painted.
        if let Some(frame) = self.recorder.session.next_frame() {
            let size = [frame.width as usize, frame.height as usize];
            // `from_rgba_unmultiplied`, not `_premultiplied`: a camera frame is
            // opaque, so alpha is 255 everywhere and the two agree — but saying
            // premultiplied would be a claim about the data that happens to be
            // true, and it stops being true the moment anything composites.
            let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.pixels);
            // Kept for the compositor, which ticks on the take's clock rather
            // than this one and will want a frame on a window frame that has
            // none of its own.
            self.recorder.camera_rgba = Some((frame.pixels.clone(), frame.width, frame.height));
            self.recorder.preview_px = egui::Vec2::new(frame.width as f32, frame.height as f32);
            match self.recorder.preview.as_mut() {
                // `set` reuses the GPU allocation; `load_texture` makes a new
                // one every frame, which at 30 fps is a texture leak with a
                // frame rate.
                Some(handle) => handle.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.recorder.preview = Some(ctx.load_texture(
                        "tangent-camera-preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }

        let camera_uid = self.app.chosen_camera_uid().map(str::to_owned);
        // `camera_running`, not `camera_format().is_some()`. The format is
        // cached at open and never becomes `None`, so testing it could not
        // detect the case this exists for — a webcam unplugged mid-session left
        // its last frame on screen looking live, indefinitely.
        let camera_open = self.recorder.session.camera_running();
        if self.recorder.session.camera_silent() {
            self.recorder
                .camera_silent_since
                .get_or_insert_with(std::time::Instant::now);
        } else {
            self.recorder.camera_silent_since = None;
        }
        if !camera_open {
            // Drop the stale picture when there is no camera behind it, or the
            // last frame of an unplugged webcam stays on screen looking live.
            self.recorder.preview = None;
        }

        // What both sides are doing, for the Audio Status panel. Pushed every
        // frame rather than fetched when the panel opens: a rate that changed
        // under you is exactly what it is there to show.
        self.app.set_audio_status(ivory_ui::recorder::AudioStatus {
            input: self.recorder.session.input_stats(),
            output: self.recorder.engine.as_ref().map(|e| {
                let o = e.output();
                (
                    o.device.clone(),
                    ivory_ui::recorder::StreamStats {
                        sample_rate: o.sample_rate,
                        channels: o.channels,
                        buffer_frames: o.buffer_frames,
                    },
                )
            }),
        });

        // The buffer size, on the edge. Reopening both streams is expensive —
        // the output takes every instrument with it — so it happens when the
        // choice CHANGES and not while a take is rolling.
        // A monitor that would not open a moment ago. See `engine_retry`.
        if self.recorder.engine.is_none() && self.app.recorder_band_open() {
            if let Some((at, _)) = self.recorder.engine_retry {
                if std::time::Instant::now() >= at {
                    self.start_engine(ctx);
                }
            }
        }

        let want_buffer = self.app.buffer_frames();
        if self.recorder.buffer_open != want_buffer && !self.recorder.session.is_recording() {
            self.recorder.buffer_open = want_buffer;
            // Dropping the engine stops the output stream, and STARTING ONE
            // AGAIN is not optional. `start_engine` otherwise runs only on the
            // edge of the band opening, so dropping it here left the app with
            // no monitor and no instrument until the band was closed and
            // reopened — which is not a thing anybody would think to try.
            //
            // Both in the same call: the old stream's `Drop` releases the
            // device before the new one asks for it, which is the ordering
            // CoreAudio needs. The five-second instrument load still happens
            // over the following frames, announced as usual.
            self.recorder.engine = None;
            self.recorder.plugin_loaded = std::array::from_fn(|_| None);
            self.start_engine(ctx);
            self.reconcile_audio(true);
        }

        let audio_uid = self.app.chosen_audio_uid().map(str::to_owned);
        let open_name = self.recorder.session.audio_device_name().map(str::to_owned);
        // "Missing" is a chosen device that is not open, which is a different
        // thing from having chosen nothing — an interface unplugged between
        // sessions must not silently look like a choice nobody made.
        let audio_missing = audio_uid.is_some() && open_name.is_none();
        // What the one status line says, worst news first: a device that will
        // not open beats a device that is denied beats the last take's report.
        let message = self
            .recorder
            .plugin_opening
            .and_then(|slot| {
                // Named, because "loading…" with no subject is the least
                // informative thing a status line can say.
                match self.app.chosen_plugin(slot) {
                    Some(p) => Some(format!(
                        "loading {} — instruments warm up for a few seconds so \
                         the first take is not silent",
                        std::path::Path::new(p)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.to_owned())
                    )),
                    None => None,
                }
            })
            .or_else(|| self.recorder.camera_opening
            .then(|| {
                "starting the camera — this can take a few seconds on a USB \
                 webcam"
                    .to_owned()
            }))
            .or_else(|| self.recorder.session.audio_error().map(str::to_owned))
            .or_else(|| self.recorder.engine_error.clone())
            .or_else(|| self.recorder.session.camera_error().map(str::to_owned))
            .or_else(|| {
                // Only once it has had a moment: every camera delivers nothing
                // for the first few frames after `startRunning` returns.
                (self.recorder.camera_silent_since.is_some_and(|t| {
                    std::time::Instant::now().duration_since(t)
                        > std::time::Duration::from_secs(3)
                }))
                .then(|| {
                    "the camera is open but sending no picture — check Camera \
                     access in System Settings > Privacy & Security"
                        .to_owned()
                })
            })
            .or_else(|| {
                self.recorder
                    .camera_denied
                    .lock()
                    .ok()
                    .and_then(|d| d.clone())
            })
            .or_else(|| self.recorder.session.last_summary().map(|s| s.message()));

        let preview = self.recorder.preview.as_ref().map(|h| ivory_ui::recorder::Preview {
            texture: h.id(),
            size: self.recorder.preview_px,
        });
        // Computed BEFORE the mutable borrow of the app: `chosen_plugin()`
        // borrows it immutably and `recorder_state_mut()` holds it mutably.
        let engine = self.recorder.engine.as_ref();
        let slots: [ivory_ui::recorder::SlotState; ivory_ui::recorder::SLOTS] =
            std::array::from_fn(|i| {
                let loaded = engine.and_then(|e| e.plugin(i));
                ivory_ui::recorder::SlotState {
                    // The instrument's own name when it loaded; the bundle's
                    // file name when it did not, so the band can say WHICH
                    // instrument is missing rather than just that one is.
                    name: loaded.map(|p| p.class.clone()).or_else(|| {
                        self.app.chosen_plugin(i).map(|p| {
                            std::path::Path::new(p)
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| p.to_owned())
                        })
                    }),
                    missing: loaded.is_none() && self.app.chosen_plugin(i).is_some(),
                    has_editor: engine.is_some_and(|e| e.has_editor(i)),
                    editor_open: engine.is_some_and(|e| e.editor_open(i)),
                }
            });

        let state = self.app.recorder_state_mut();
        state.preview = preview;
        state.camera_name = self
            .recorder
            .session
            .camera_format()
            .map(|f| format!("{}x{} @ {:.0}fps", f.width, f.height, f.fps))
            .or_else(|| camera_uid.clone());
        state.camera_missing = camera_uid.is_some() && !camera_open;
        state.state = self.recorder.session.state();
        state.elapsed_s = self.recorder.session.elapsed();
        state.meters = self.recorder.session.meters();
        state.dest = shorten_home(&root);
        state.folder_preview = take::folder_name(
            &take::WallTime::now_utc(),
            name.as_deref().and_then(take::sanitise_slug).as_deref(),
        );
        state.audio_name = open_name.or(audio_uid);
        state.audio_missing = audio_missing;
        state.disk_minutes = self
            .recorder
            .disk_bytes
            .and_then(|b| ivory_ui::recorder::minutes_on_disk(b, &spec));
        state.slots = slots;
        state.message = message;
        state.clip_warning = self.recorder.session.clipped();
        // Cleared the moment a new take starts: "re-export the last take" stops
        // being a sensible offer once there is a take in progress.
        state.last_take_folder = (!self.recorder.session.is_recording())
            .then(|| {
                self.recorder
                    .session
                    .last_summary()
                    .filter(|s| s.problem.is_none())
                    .map(|s| s.folder.clone())
            })
            .flatten();
    }

    /// Everything that must happen OUTSIDE a frame: opening devices, raising
    /// native panels, creating directories.
    fn after_frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        use ivory_ui::recorder::RecorderRequest as R;

        // Dev hook, alongside IVORY_INLINE and IVORY_DEMO_NOTES: open slot 0's
        // editor once, a second in, without anybody clicking. It is how a
        // window-interaction bug gets reproduced and bisected from a script;
        // driving it by hand makes every measurement a different measurement.
        //   IVORY_OPEN_EDITOR=1 dist/Tangent.app/Contents/MacOS/tangent
        if !self.recorder.dev_editor_done
            && std::env::var("IVORY_OPEN_EDITOR").as_deref() == Ok("1")
        {
            let due = self
                .recorder
                .dev_editor_at
                .get_or_insert_with(|| std::time::Instant::now() + DEV_EDITOR_DELAY);
            if std::time::Instant::now() >= *due {
                self.recorder.dev_editor_done = true;
                if let Some(e) = self.recorder.engine.as_mut() {
                    match e.open_editor(0) {
                        Ok(()) => eprintln!("IVORY_OPEN_EDITOR: slot 1 editor opened"),
                        Err(err) => eprintln!("IVORY_OPEN_EDITOR: {err}"),
                    }
                }
            }
            ctx.request_repaint();
        }

        // The instrument's own window, if it has one open. Polled rather than
        // notified because the user closes it with the OS's close button, which
        // the plugin's view knows about and we only find out by asking.
        if let Some(e) = self.recorder.engine.as_mut() {
            let was: Vec<bool> = (0..ivory_ui::recorder::SLOTS).map(|i| e.editor_open(i)).collect();
            e.poll_editor();
            // An editor that has just closed is a preset that has just been
            // chosen. Save then, rather than only at quit, because the gap
            // between the two is where a force-quit loses the sound.
            let closed = (0..ivory_ui::recorder::SLOTS)
                .any(|i| was[i] && !self.recorder.engine.as_ref().is_some_and(|e| e.editor_open(i)));
            if closed {
                self.save_plugin_states();
            }
        }

        // The always-on MIDI tap, drained whether or not a take is running, and
        // fanned out to the monitor engine in the SAME drain — the tap is a
        // queue, so two independent drains would give each message to one
        // consumer and starve the other.
        let engine = self.recorder.engine.as_ref();
        self.recorder.session.pump_midi(|t, bytes| {
            if let Some(e) = engine {
                e.send_midi(t, bytes);
            }
        });

        // Opening the BAND opens the input, not pressing Record — the meter
        // has to be live before arming, which is what kills the "I recorded
        // silence" failure class.
        let open = self.app.recorder_band_open();
        if open != self.recorder.band_was_open {
            self.recorder.band_was_open = open;
            if open {
                self.start_engine(ctx);
                self.reconcile_audio(true);
                self.reconcile_camera(true, ctx);
            } else {
                self.recorder.session.close_input();
                self.recorder.session.close_camera();
                self.recorder.camera_opening = false;
                // Dropping it stops the output stream and unloads the plugin,
                // which is what "the band is closed" should mean: no device
                // held, no third-party code resident.
                self.recorder.engine = None;
                self.recorder.plugin_loaded = [const { None }; ivory_ui::recorder::SLOTS];
            }
        } else if open {
            self.reconcile_audio(false);
            self.reconcile_camera(false, ctx);
            self.reconcile_plugin(ctx);
            self.push_monitor_settings();
            self.push_take_source();
        }

        let root = self.app.record_root();
        let name = self.app.take_name().map(str::to_owned);

        // The count-in ends on the beat the player HEARD, not on the frame that
        // noticed it had. The audio thread knows that instant exactly — it
        // scheduled the click and it knows the device's output delay — and the
        // UI thread can only ever be a frame late and a buffer short.
        if let Some(downbeat) = self
            .recorder
            .engine
            .as_ref()
            .filter(|e| e.count_in_done())
            .and_then(|e| e.count_in_downbeat_ns())
        {
            // **Once per count-in, not once per frame.** `count_in_done` is a
            // LATCH: it stays true after the count finishes and is only cleared
            // when the next one starts. So this armed the session with the same
            // downbeat on every frame for the rest of the session — and the
            // next take that began WITHOUT a count-in took that stale instant
            // as its T0. With "record the count-in into the take" on, every
            // take starts with a count-in length of zero, so every take after
            // the first would have been timestamped from the first one's
            // downbeat: minutes of offset between the audio and the `.mid`.
            if self.recorder.armed_downbeat != Some(downbeat) {
                self.recorder.armed_downbeat = Some(downbeat);
                self.recorder.session.arm_at(downbeat);
            }
        }
        // The count-in, which is the one thing here that animates with no input
        // at all — so it also has to ask for the next frame.
        if self.recorder.session.tick(&root, name.as_deref()) {
            ctx.request_repaint();
        }
        // And a repaint while anything is live, or the meter and the clock
        // update only when the mouse moves.
        if self.recorder.session.is_recording() || open {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }

        if let Some(request) = self.app.take_directory_request() {
            let mut dialog = rfd::FileDialog::new().set_title(&request.title);
            if let Some(start) = request.start_at.filter(|p| p.exists()) {
                dialog = dialog.set_directory(start);
            }
            if let Some(dir) = dialog.pick_folder() {
                // The tick is left where the user had it. Choosing a folder is
                // not a statement about whether to go on choosing it, and
                // silently ticking "use this by default" because somebody
                // picked a folder once is how a temporary destination becomes
                // permanent without anyone deciding it should.
                let remember = self.app.record_dir_is_default();
                self.app.set_record_dir(dir, remember);
            }
        }

        // The take's video, on the EDGES of the session's own state. Placed
        // after the request loop so that a Record pressed this frame is already
        // rolling by the time this asks, and a Stop pressed this frame is
        // already stopped — the alternative is a video that starts and ends one
        // window frame late at both ends.
        #[cfg(target_os = "macos")]
        {
            // **`is_writing`, not `is_recording`.** The latter is true through
            // the COUNT-IN, and during a count-in there is no take folder yet —
            // so `begin_video` found nothing to write to, gave up, and set the
            // flag that stops it trying again. Anybody with a count-in got no
            // video at all, every time, silently. It is also the right rule on
            // its own terms: the bars before the downbeat are deliberately not
            // in the audio, and they have no business being in the video.
            let writing = self.recorder.session.state().is_writing();
            if writing {
                self.begin_video(frame);
                self.pump_video();
            } else {
                self.end_video();
                self.recorder.video_tried = false;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = frame;

        if let Some(path) = self.app.take_reveal_request() {
            reveal(&path);
        }
        // The automatic one, at most once per finished take. The marker is
        // updated whether or not the tick is on, so turning it on after a take
        // does not immediately open a window for a recording already made.
        if !self.recorder.session.is_recording() {
            if let Some(folder) = self
                .recorder
                .session
                .last_summary()
                .filter(|s| s.problem.is_none())
                .map(|s| s.folder.clone())
                .filter(|f| !f.is_empty())
            {
                if self.recorder.seen_take.as_deref() != Some(folder.as_str()) {
                    self.recorder.seen_take = Some(folder.clone());
                    if self.app.record_open_when_done() {
                        reveal(&self.app.record_root().join(&folder));
                    }
                }
            }
        }

        while let Some(request) = self.app.take_recorder_request() {
            match request {
                R::Toggle => {
                    // The click counts the take in, on the audio thread's own
                    // sample clock. Started here, at the press, so the first
                    // beat lands immediately rather than a frame later.
                    let beats = self.app.count_in_beats();
                    let in_take = self.app.count_in_in_take();
                    if let Some(e) = self.recorder.engine.as_ref() {
                        // The click's own switch for the count, which is not
                        // `metronome_in_take` — see `Shared::count_in_in_take`.
                        e.set_count_in_in_take(in_take);
                        if !self.recorder.session.is_recording() && beats > 0 {
                            e.start_count_in(beats, self.app.tempo_bpm());
                        } else {
                            e.cancel_count_in();
                        }
                    }
                    // A fresh take starts with a clean clip latch on the
                    // instrument's meter, exactly as `LevelTracker::arm` does
                    // for the input's — a clip from the last take reported
                    // against this one is worse than no indicator.
                    if let Some(e) = self.recorder.engine.as_ref() {
                        e.clear_clip();
                    }
                    let spec = self.app.export_spec();
                    // **Zero when the count-in goes INSIDE the take.** The
                    // session's count-in is a delay before the file opens; the
                    // engine's is the click. Passing zero here starts writing
                    // at the press while the click counts on regardless, which
                    // is exactly "start instantly with the count-in in the
                    // export" — the count is at the head of the file to trim to
                    // or to keep.
                    let wait = if in_take { 0 } else { beats };
                    self.recorder.session.toggle(&root, name.as_deref(), wait, spec);
                }
                R::Stop => self.recorder.session.stop(),
                R::OpenPluginEditor(slot) => {
                    // The plugin's own window, created here rather than in the
                    // frame: VST3 requires the main thread and AppKit will not
                    // have a window built while an egui frame is on the stack.
                    // The engine owns it, because the engine owns the plugin
                    // the view belongs to.
                    if let Some(e) = self.recorder.engine.as_mut() {
                        // One row, two names, one action: open it, or close the
                        // one that is open. A second menu row for closing a
                        // window that has its own close button is clutter.
                        if e.editor_open(slot) {
                            e.close_editor(slot);
                        } else if let Err(err) = e.open_editor(slot) {
                            self.recorder.engine_error =
                                Some(format!("could not open the instrument window: {err}"));
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
    }

    /// Write every loaded slot's plugin state beside the settings.
    ///
    /// Called on quit and whenever an editor closes — the second matters more
    /// than the first, because closing the editor is the moment right after
    /// somebody chose a preset, and it is also the moment they are most likely
    /// to then force-quit or unplug something.
    fn save_plugin_states(&mut self) {
        for slot in 0..ivory_ui::recorder::SLOTS {
            let Some(bundle) = self.app.chosen_plugin(slot).map(str::to_owned) else {
                continue;
            };
            if let Some(state) = self
                .recorder
                .engine
                .as_ref()
                .and_then(|e| e.save_slot_state(slot))
            {
                write_state(slot, &bundle, &state);
            }
        }
    }

    /// Start the monitor output, once, when the band opens.
    ///
    /// A failure here is not fatal and must not be: a machine with no output
    /// device, or one another app holds exclusively, still has a perfectly good
    /// chord display and a perfectly good recorder. The band says what happened
    /// and everything else carries on.
    fn start_engine(&mut self, ctx: &egui::Context) {
        if self.recorder.engine.is_some() {
            return;
        }
        match crate::instrument::Engine::start_sized(
            None,
            self.recorder.session.timebase(),
            self.app.buffer_frames(),
        ) {
            Ok(e) => {
                self.recorder.engine = Some(e);
                self.recorder.engine_error = None;
                self.recorder.engine_retry = None;
                self.recorder.plugin_loaded = [const { None }; ivory_ui::recorder::SLOTS];
                self.push_monitor_settings();
                // Announces rather than loads on this frame: the band has just
                // appeared and a remembered instrument would otherwise freeze
                // it for five seconds before it had drawn once.
                self.reconcile_plugin(ctx);
            }
            Err(e) => {
                self.recorder.engine_error = Some(format!("no audio output: {e}"));
                // Try again shortly, a few times. See `engine_retry`.
                let left = self.recorder.engine_retry.map_or(ENGINE_TRIES, |(_, n)| n);
                self.recorder.engine_retry = (left > 0).then(|| {
                    (
                        std::time::Instant::now() + ENGINE_RETRY_AFTER,
                        left.saturating_sub(1),
                    )
                });
            }
        }
    }

    /// Copy the faders, the click and the tempo into the audio thread.
    ///
    /// Every frame, unconditionally. These are all atomic stores of a float or
    /// a bool behind a smoothing ramp, so writing an unchanged value costs
    /// nothing — and a change-detection cache here would be a second copy of
    /// the settings to get out of step with the first.
    fn push_monitor_settings(&mut self) {
        // The SESSION's copy first, and outside the engine gate. It is what the
        // count-in's on-screen beat and the `.mid`'s tempo map are derived
        // from, and neither has anything to do with an output device — so a
        // machine with no monitor (or one another app holds) would have kept
        // counting and writing 4/4 while the band showed 6/8.
        self.recorder.session.set_meter(self.app.time_signature());
        let Some(e) = self.recorder.engine.as_ref() else {
            return;
        };
        let gains = self.app.gains();
        for (slot, g) in gains.slots.iter().enumerate() {
            e.set_slot_gain(slot, *g);
        }
        e.set_metronome_gain(gains.metronome);
        e.set_metronome_enabled(self.app.metronome_on());
        e.set_metronome_in_take(self.app.metronome_in_take());
        e.set_tempo(self.app.tempo_bpm());
        // The signature drives both halves of the click: which beat is accented
        // and how long a beat lasts. In 6/8 those are "every sixth" and "half a
        // quarter" — get the second wrong and the count-in is twice as long as
        // the bar it is counting.
        // NOT while a take is running. `Session::set_meter` refuses mid-take —
        // a `.mid` whose bar lines change halfway through is a file nobody can
        // edit — and pushing it to the engine anyway would move the click and
        // the accent while the countdown and the file kept the old meter. One
        // setting must not have two live values.
        if !self.recorder.session.is_recording() {
            let sig = self.app.time_signature();
            e.set_meter(u32::from(sig.beats), u32::from(sig.unit));
        }
    }

    /// Decide what the next take is made of, from what is actually available.
    fn push_take_source(&mut self) {
        let plugin = self
            .recorder
            .engine
            .as_ref()
            .is_some_and(crate::instrument::Engine::any_plugin_loaded);
        let input = self.recorder.session.audio_device_name().is_some();
        let want = crate::record::TakeSource::resolve(
            self.app.audio_source_setting(),
            plugin,
            input,
        );
        if want != self.recorder.session.source() {
            self.recorder.session.set_source(want);
        }
    }

    /// Load or unload the instrument the settings name.
    ///
    /// **Blocking**, like the camera: `Module::open` runs a third-party
    /// library's initialiser and `Instance::create` can take seconds on a
    /// sampler. Hence after the frame, never inside one.
    fn reconcile_plugin(&mut self, ctx: &egui::Context) {
        // ONE slot per call. Loading blocks for about five seconds, so three
        // stale slots would freeze the window for fifteen; taking them one
        // frame at a time keeps the band alive and lets the status line name
        // each instrument as it arrives.
        let Some(slot) = (0..ivory_ui::recorder::SLOTS).find(|i| {
            self.app.chosen_plugin(*i).map(str::to_owned) != self.recorder.plugin_loaded[*i]
        }) else {
            return;
        };
        let wanted = self.app.chosen_plugin(slot).map(str::to_owned);

        // **No engine, nothing to load into.** Checked BEFORE announcing, and
        // that ordering is the whole of a bug that looked like a failing
        // plugin: announcing first meant frame one set "loading…", frame two
        // found no engine and returned WITHOUT settling `plugin_loaded`, and
        // frame three announced again — for ever, at sixty frames a second,
        // each one asking for a repaint. What the user sees is an instrument
        // flickering between loading and not, and it is not the instrument's
        // fault at all.
        if self.recorder.engine.is_none() {
            self.recorder.plugin_opening = None;
            return;
        }

        // Announce first, act next frame — but only when there is a wait to
        // explain. Unloading is instant, so making the user watch a frame of
        // "loading…" in order to REMOVE an instrument would be silly.
        if wanted.is_some() && self.recorder.plugin_opening != Some(slot) {
            self.recorder.plugin_opening = Some(slot);
            ctx.request_repaint();
            return;
        }
        self.recorder.plugin_opening = None;
        let Some(e) = self.recorder.engine.as_mut() else {
            return;
        };
        // The editor FIRST, in both branches: it is a view onto THIS
        // instrument, and a window still attached to a plugin that has been
        // terminated is a use-after-free with a title bar.
        e.close_editor(slot);
        match &wanted {
            None => {
                e.unload_plugin(slot);
                self.recorder.engine_error = None;
            }
            Some(path) => match e.load_plugin_with_state(
                slot,
                std::path::Path::new(path),
                None,
                saved_state(slot, path).as_deref(),
            ) {
                Ok(_) => {
                    self.recorder.engine_error = None;
                    // The take has to be able to RECORD what it can now hear.
                    // Taken once for the engine's lifetime — the tap belongs to
                    // the engine rather than to any one instrument, so it
                    // survives a slot changing and a take already rolling never
                    // changes width.
                    if let Some(tap) =
                        self.recorder.engine.as_mut().and_then(|e| e.take_recorder_tap())
                    {
                        self.recorder.session.set_plugin_tap(Some(tap));
                    }
                }
                Err(err) => {
                    // The path is REMEMBERED even though it failed. A plugin
                    // that will not load today because its licence server was
                    // unreachable should still be the chosen one tomorrow, and
                    // the band shows it as `Missing` rather than forgetting it.
                    self.recorder.engine_error =
                        Some(format!("could not load the instrument: {err}"));
                }
            },
        }
        // Settled either way, so a plugin that refuses to load is not retried
        // sixty times a second for the rest of the session.
        self.recorder.plugin_loaded[slot] = wanted;
    }

    /// Open the camera the user asked for, if it is not already open.
    ///
    /// **`open_camera` blocks for 300-800 ms** (over two seconds for a
    /// Continuity Camera), which is why this runs after the frame and why
    /// opening the band rather than pressing Record is what triggers it.
    ///
    /// Unlike the audio path there is no "system default": a camera nobody
    /// asked for must never be opened, because opening one turns on a light on
    /// the front of the machine.
    fn reconcile_camera(&mut self, force: bool, ctx: &egui::Context) {
        let sel = crate::devices::selection(&self.recorder.camera);
        if !sel.is_stale() && !force {
            return;
        }
        let wanted = sel.wanted;
        // Announce first, act next frame — but only when there is something to
        // wait for. Closing is instant, so making the user watch a frame of
        // "starting the camera…" in order to turn one OFF would be silly.
        if wanted.is_some() && !self.recorder.camera_opening {
            self.recorder.camera_opening = true;
            ctx.request_repaint();
            return;
        }
        self.recorder.camera_opening = false;
        // No `if wanted.is_none() { return }` guard here, and the guard that
        // used to be here was a bug: choosing "None — record without video"
        // left the selection stale forever, so the camera went on running with
        // its light on and its preview updating after the user said stop.
        // `open_camera(None)` closes it, which is what None means.
        self.recorder.session.open_camera(wanted.as_deref());
        let name = self
            .recorder
            .session
            .camera_format()
            .map(|f| format!("{}x{}", f.width, f.height));
        crate::devices::settle(
            &self.recorder.camera,
            wanted,
            name,
            self.recorder.session.camera_error().map(str::to_owned),
        );
    }

    /// Open the audio input the user asked for, if it is not already open.
    fn reconcile_audio(&mut self, force: bool) {
        let stale = {
            let sel = crate::devices::selection(&self.recorder.audio);
            sel.is_stale()
        };
        if !stale && !force {
            return;
        }
        match crate::devices::audio_selection(&self.recorder.audio) {
            Some(selection) => self
                .recorder
                .session
                .open_input(&selection, self.app.buffer_frames()),
            // The user picked "None — record MIDI only". Mapping that to the
            // system default (which is what happened before `explicit` existed)
            // opened the built-in microphone and put its name in the band.
            None => self.recorder.session.close_input(),
        }
        let opened = crate::devices::selection(&self.recorder.audio).wanted;
        crate::devices::settle(
            &self.recorder.audio,
            opened,
            self.recorder.session.audio_device_name().map(str::to_owned),
            self.recorder.session.audio_error().map(str::to_owned),
        );
    }
}

/// Where one slot's plugin state lives.
///
/// A sidecar file rather than a key in `settings.json`: Pianoteq's state is
/// **41,233 bytes**, so three slots would put ~165 KB of base64 into a file a
/// human is expected to be able to open and read. It sits beside the settings
/// so it travels with them.
#[cfg(feature = "recorder")]
fn state_path(slot: usize) -> std::path::PathBuf {
    let dir = Settings::path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    dir.join(format!("plugin-state-{slot}.bin"))
}

/// The saved state for `slot`, but only if it belongs to `bundle`.
///
/// The bundle path is written into the file and checked on the way back,
/// because handing Pianoteq's state to Piano V3 is not a preset, it is
/// arbitrary bytes to a `setState` that will believe them. `ivory-host`'s
/// container catches corruption; nothing but this catches *the wrong plugin*.
#[cfg(feature = "recorder")]
fn saved_state(slot: usize, bundle: &str) -> Option<Vec<u8>> {
    let raw = std::fs::read(state_path(slot)).ok()?;
    let split = raw.iter().position(|b| *b == 0)?;
    let owner = std::str::from_utf8(&raw[..split]).ok()?;
    (owner == bundle).then(|| raw[split + 1..].to_vec())
}

#[cfg(feature = "recorder")]
fn write_state(slot: usize, bundle: &str, state: &[u8]) {
    let mut out = Vec::with_capacity(bundle.len() + 1 + state.len());
    out.extend_from_slice(bundle.as_bytes());
    out.push(0);
    out.extend_from_slice(state);
    // Best effort. A preset that could not be saved is a preset to choose
    // again, and refusing to quit over it would be worse.
    let _ = std::fs::write(state_path(slot), out);
}

/// `/Users/x/Movies/Tangent` reads as `~/Movies/Tangent`.
///
/// Not cosmetic at this width: the band's destination line has room for about
/// forty characters, and a home directory eats a quarter of them saying nothing
/// the user does not already know.
#[cfg(feature = "recorder")]
fn shorten_home(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().into_owned();
    let Some(home) = dirs::home_dir() else {
        return text;
    };
    let home = home.to_string_lossy();
    match text.strip_prefix(home.as_ref()) {
        Some(rest) => format!("~{rest}"),
        None => text,
    }
}

impl DesktopApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        cli_port: Option<String>,
    ) -> Self {
        // Dev switch: run the DESKTOP binary through the PLUGIN's capability
        // set, so the in-canvas menu and dialogs can be looked at, screenshot
        // and debugged in a window — without a DAW, a rebuild, an installer
        // and a plugin rescan between each attempt.
        //
        //   IVORY_INLINE=1 /Applications/Tangent.app/Contents/MacOS/tangent
        //
        // Environment-gated, so a normal launch cannot reach it. It is a
        // faithful test of the path: the same `Caps::PLUGIN` the plugin uses,
        // which also means the window will not resize itself.
        //   IVORY_INLINE=menu also opens the menu on the first frame.
        let caps = if matches!(
            std::env::var("IVORY_INLINE").as_deref(),
            Ok("1") | Ok("menu")
        ) {
            eprintln!("IVORY_INLINE=1: running with the plugin's capabilities");
            Caps::PLUGIN
        } else {
            Caps::DESKTOP
        };
        let mut app = IvoryApp::new(&cc.egui_ctx, settings, caps);
        let mut device = DeviceMidi::new(cc.egui_ctx.clone());
        // Grabbed BEFORE the connection is made and before the session exists,
        // because everything that stamps a time has to share one epoch. Two
        // `Timebase::new()` calls would silently place the MIDI and the audio
        // in two different worlds, and the symptom would be a constant offset
        // in every take that no test covers.
        #[cfg(feature = "recorder")]
        let (tap, timebase) = (device.tap(), device.timebase());
        // `-p NAME` beats the priority chain, and a bad name is not fatal: the
        // app opens with no MIDI, which is the same outcome as no device.
        match cli_port {
            Some(name) => {
                if let Err(e) = device.connect(&name, app.midi_sender()) {
                    eprintln!("could not open MIDI port {name:?}: {e}");
                }
            }
            None => device.auto_connect(app.midi_sender()),
        }
        app.set_ports(Some(Box::new(device)));

        #[cfg(feature = "recorder")]
        let recorder = {
            let (inputs, audio) = crate::devices::AudioInputs::new();
            let (cams, camera, camera_denied) = crate::devices::Cameras::new();
            // Seed from what the settings file remembers, or the reconciler —
            // which only ever acts on a difference — would never open the
            // chosen device and the app would look like it had forgotten.
            crate::devices::restore(
                &audio,
                app.chosen_audio_uid(),
                app.audio_explicitly_off(),
            );
            // No `explicitly_off` for the camera: absent already means no
            // camera there, because opening one turns on a light and a camera
            // nobody asked for must never be opened.
            crate::devices::restore(&camera, app.chosen_camera_uid(), false);
            app.set_capture_devices(Some(Box::new(inputs)));
            app.set_cameras(Some(Box::new(cams)));
            // Every installed VST3, by path and file name. This is a DIRECTORY
            // LISTING, not a scan: nothing is opened, so it costs milliseconds
            // even with 112 of them, and no plugin gets the chance to crash the
            // process before the window has appeared. A bundle is opened only
            // when the user picks it.
            app.set_plugin_list(ivory_host::discover());
            Recorder {
                session: crate::record::Session::new(tap, timebase),
                audio,
                camera,
                camera_denied,
                engine: None,
                engine_error: None,
                plugin_loaded: [const { None }; ivory_ui::recorder::SLOTS],
                plugin_opening: None,
                camera_opening: false,
                camera_silent_since: None,
                preview: None,
                preview_px: egui::Vec2::ZERO,
                band_was_open: false,
                disk_checked_at: None,
                disk_bytes: None,
                dev_editor_at: None,
                #[cfg(target_os = "macos")]
                video: None,
                video_tried: false,
                camera_rgba: None,
                engine_retry: None,
                armed_downbeat: None,
                buffer_open: None,
                seen_take: None,
                dev_editor_done: false,
            }
        };

        Self {
            app,
            #[cfg(feature = "recorder")]
            recorder,
            splash: Some(Splash {
                since: std::time::Instant::now(),
                done_at: None,
            }),
        }
    }
}

impl eframe::App for DesktopApp {
    /// eframe hands over a Context; everything below wants a Ui, and
    /// `IvoryApp::frame` is the bridge all three hosts share.
    ///
    /// The recorder brackets it: state in before, requests out after. Nothing
    /// that opens a device, raises a native panel or creates a directory
    /// happens between those two lines.
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(feature = "recorder")]
        self.fill_recorder_state(ctx);
        self.app.frame(ctx);
        #[cfg(feature = "recorder")]
        self.after_frame(ctx, frame);
        self.paint_splash(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        IvoryApp::CLEAR_COLOR
    }

    /// A take still running when the window is closed is FINISHED, not
    /// abandoned.
    ///
    /// Without this the writer thread is torn down with the process and the
    /// `.wav` keeps the placeholder sizes in its header — a file most players
    /// treat as zero-length. Somebody who left the recorder running for a
    /// practice session and then quit would lose the whole thing.
    #[cfg(feature = "recorder")]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.recorder.session.stop();
        self.save_plugin_states();
    }
}

/// Show `path` in the platform's file manager.
///
/// Best effort and deliberately silent on failure. There is no useful thing to
/// tell somebody whose file manager did not open — the take is written either
/// way, the folder is named on screen, and an error banner over the Recorder
/// would be reporting a problem with the CONVENIENCE as though it were a
/// problem with the recording.
///
/// `spawn` and not `status`: waiting on Finder or Explorer would block the UI
/// thread for as long as the window takes to appear.
fn reveal(path: &std::path::Path) {
    // A folder that does not exist yet is not an error either. The destination
    // is created when the first take is written, so pressing SHOW before ever
    // recording would otherwise open a window onto nothing.
    if !path.exists() {
        return;
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    let _ = cmd.spawn();
}

// ───────────────────────────────────────────────────────────────────────────
// The take's video
// ───────────────────────────────────────────────────────────────────────────

/// The compositor and the encoder for a take that is being filmed.
///
/// Both live on the UI thread, and that is the whole thread design: the
/// compositor must be here because it paints the app, and the encoder is here
/// because moving IT here costs 384 kB a second of audio crossing a channel,
/// where moving the compositor to the writer thread would cost 250 MB a second
/// of composited frames going the other way.
#[cfg(all(feature = "recorder", target_os = "macos"))]
struct TakeVideo {
    compositor: crate::composite::Compositor,
    encoder: ivory_record::encode::Encoder,
    /// The next frame index to produce. The video's clock is the TAKE's clock:
    /// frame `n` is presented at `n / fps` seconds after the take started,
    /// whatever the camera has managed to deliver by then.
    next: u64,
    fps: u32,
    layout: ivory_ui::recorder::Layout,
    shows: ivory_ui::recorder::DisplayShows,
    camera: bool,
    display: bool,
    path: std::path::PathBuf,
    /// Frames the compositor or the encoder refused, so the summary can say so
    /// rather than the user counting them in the finished file.
    failed: u64,
}

#[cfg(all(feature = "recorder", target_os = "macos"))]
impl DesktopApp {
    /// Start filming, if this take is meant to be filmed.
    ///
    /// Every refusal is a message rather than a silent skip: a take that was
    /// supposed to produce an `.mp4` and did not is exactly the failure that
    /// wastes a performance.
    fn begin_video(&mut self, frame: &eframe::Frame) {
        let spec = self.app.export_spec();
        if !spec.video.wants_video() || self.recorder.video.is_some() || self.recorder.video_tried {
            return;
        }
        // The folder FIRST, and the flag only after it. Setting the flag before
        // this check is what turned "the take is not ready yet" into "this take
        // will never have video" — a one-frame condition becoming permanent.
        let Some(dir) = self.recorder.session.take_dir().map(|d| d.to_path_buf()) else {
            return;
        };
        self.recorder.video_tried = true;
        // The camera's own size, for `MatchCamera` and for nothing else.
        let cam = self
            .recorder
            .session
            .camera_format()
            .map(|f| (f.width, f.height));
        let (w, h) = spec.resolution.pixels().or(cam).unwrap_or((1920, 1080));
        let want_camera = spec.composite.camera && self.recorder.session.camera_running();
        let want_display = spec.composite.display && spec.composite.shows.any();
        if !want_camera && !want_display {
            self.recorder.engine_error =
                Some("the video has neither the camera nor the display in it".to_owned());
            return;
        }
        let path = dir.join("take.mp4");
        let video = ivory_record::encode::VideoSpec {
            width: w,
            height: h,
            fps: spec.fps.max(1),
        };
        // The audio track exists only when the writer is actually sending
        // samples — a take with the `.wav` unticked has no audio to mux, and a
        // silent video is a legitimate request in its own right.
        let audio = spec
            .composite
            .audio
            .then(|| self.recorder.session.video_audio_spec())
            .flatten()
            .map(|(rate, channels)| ivory_record::encode::AudioSpec {
                sample_rate: rate,
                channels,
            });
        let compositor =
            match crate::composite::Compositor::new(frame.wgpu_render_state(), video.width, video.height) {
                Ok(c) => c,
                Err(e) => {
                    self.recorder.engine_error = Some(format!("no video this take: {e}"));
                    return;
                }
            };
        // The offscreen context needs the app's fonts or every chord name in
        // the video renders in egui's default face.
        self.app.install_fonts(compositor.context());
        let encoder = match ivory_record::encode::Encoder::create(&path, video, audio) {
            Ok(e) => e,
            Err(e) => {
                self.recorder.engine_error = Some(format!("no video this take: {e}"));
                return;
            }
        };
        self.recorder.video = Some(TakeVideo {
            compositor,
            encoder,
            next: 0,
            fps: video.fps,
            layout: spec.composite.layout,
            shows: spec.composite.shows,
            camera: want_camera,
            display: want_display,
            path,
            failed: 0,
        });
    }

    /// Produce every video frame that is due, and drain the audio behind it.
    ///
    /// **Ticked from the take's own elapsed time**, not from the window's frame
    /// rate. The window may be drawing at 60, or at 8 while a plugin editor is
    /// dragging; neither may change how many frames a second the video has.
    fn pump_video(&mut self) {
        // Taken OUT for the duration, not borrowed. `&mut self.recorder.video`
        // and `&self.recorder.session` are both borrows of `self.recorder`, and
        // this function needs the encoder mutably while reading the session and
        // the app.
        let Some(mut v) = self.recorder.video.take() else {
            return;
        };
        // The audio first, so a long video stall cannot leave the encoder's two
        // inputs far apart in time — AVAssetWriter buffers the gap in memory.
        if let Some(rx) = self.recorder.session.video_audio() {
            while let Ok(chunk) = rx.try_recv() {
                if let Err(e) = v.encoder.push_audio(&chunk.samples, chunk.first_frame) {
                    self.recorder.engine_error = Some(e);
                    break;
                }
            }
        }
        let elapsed = self.recorder.session.elapsed();
        // A cap per window frame. Without it, a machine that stalls for two
        // seconds tries to composite sixty frames in one go and stalls for two
        // more — the classic spiral of death.
        const MAX_PER_FRAME: u32 = 3;
        let mut made = 0;
        while made < MAX_PER_FRAME && (v.next as f64) < elapsed * f64::from(v.fps) {
            let pts = (v.next as i64 * 1_000_000_000) / i64::from(v.fps);
            let frame = self.recorder.camera_rgba.as_ref().map(|(px, w, h)| (px.as_slice(), *w, *h));
            match v
                .compositor
                .frame(&self.app, v.layout, v.shows, v.camera, v.display, frame)
            {
                Ok(bgra) => {
                    if v.encoder.push(bgra, pts).is_err() {
                        v.failed += 1;
                    }
                }
                Err(_) => v.failed += 1,
            }
            v.next += 1;
            made += 1;
        }
        self.recorder.video = Some(v);
    }

    /// Close the video file. Must happen, or the container has no index.
    fn end_video(&mut self) {
        let Some(mut v) = self.recorder.video.take() else {
            return;
        };
        // One last drain, for the samples the writer flushed at Stop. Without
        // it the video's audio is a poll interval shorter than the `.wav`.
        if let Some(rx) = self.recorder.session.video_audio() {
            while let Ok(chunk) = rx.try_recv() {
                let _ = v.encoder.push_audio(&chunk.samples, chunk.first_frame);
            }
        }
        let dropped = v.encoder.dropped_not_ready() + v.failed;
        let path = v.path.clone();
        match v.encoder.finish() {
            Ok(()) => {
                if dropped > 0 {
                    self.recorder.engine_error = Some(format!(
                        "the video is complete, but {dropped} frames were dropped — \
                         this machine could not composite and encode in real time"
                    ));
                }
            }
            Err(e) => {
                self.recorder.engine_error = Some(format!("the video could not be finished: {e}"));
                // A half-written mp4 has no index and no player will open it.
                // Removing it is kinder than leaving a file that looks like a
                // take and is not one.
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The launch splash
// ───────────────────────────────────────────────────────────────────────────

/// How long the splash stays up at minimum.
///
/// Long enough that a fast launch does not FLASH — a splash that appears and
/// vanishes inside two frames is a glitch, not a loading screen — and short
/// enough that it is never the thing keeping anybody waiting.
const SPLASH_MIN: std::time::Duration = std::time::Duration::from_millis(600);
/// And the longest it may EVER stay up.
///
/// The cap is the important half. Everything the splash waits on is a device,
/// and a device that never answers is exactly the case where the user must not
/// be left staring at a wordmark with no way through. After this it lifts
/// whether or not anything is ready, and the band underneath says what is still
/// going on — which it was going to do anyway.
const SPLASH_MAX: std::time::Duration = std::time::Duration::from_secs(12);
/// The fade out, once it has been earned.
const SPLASH_FADE: std::time::Duration = std::time::Duration::from_millis(280);

struct Splash {
    since: std::time::Instant,
    /// When everything it was waiting for finished, so the fade can run from
    /// there rather than from the moment the window opened.
    done_at: Option<std::time::Instant>,
}

impl DesktopApp {
    /// Paint the splash, and decide when it goes.
    ///
    /// Drawn on the FOREGROUND layer after the app's own frame, so it covers
    /// everything including any dialog that opened underneath it — a Welcome
    /// card half-visible through a loading screen is the sort of detail that
    /// makes an app feel unfinished.
    fn paint_splash(&mut self, ctx: &egui::Context) {
        let Some(splash) = self.splash.as_mut() else {
            return;
        };
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(splash.since);

        // What is still being waited on, and what to say about it.
        #[cfg(feature = "recorder")]
        let (instrument, camera) = (
            self.recorder.plugin_opening.is_some(),
            self.recorder.camera_opening,
        );
        #[cfg(not(feature = "recorder"))]
        let (instrument, camera) = (false, false);

        let busy = instrument || camera;
        // "Ready" is a minimum time AND nothing outstanding — or the cap.
        if !busy && elapsed >= SPLASH_MIN && splash.done_at.is_none() {
            splash.done_at = Some(now);
        }
        if elapsed >= SPLASH_MAX && splash.done_at.is_none() {
            splash.done_at = Some(now);
        }

        let fade = match splash.done_at {
            None => 1.0,
            Some(at) => {
                let gone = now.duration_since(at).as_secs_f32();
                1.0 - (gone / SPLASH_FADE.as_secs_f32()).clamp(0.0, 1.0)
            }
        };
        if fade <= 0.0 {
            self.splash = None;
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("tangent-splash"),
        ));
        let rect = ctx.screen_rect();
        ivory_ui::splash::draw(
            &painter,
            rect,
            ivory_ui::splash::status(instrument, camera),
            fade,
        );
        // While it is up, the window must keep repainting — nothing else is
        // asking it to, and a splash that freezes mid-fade because no input
        // arrived is worse than none.
        ctx.request_repaint();
    }
}

