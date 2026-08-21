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
    /// What was last ASKED for on the effects bus, whether or not it loaded.
    /// Settled either way, so a plugin that refuses is not retried every frame.
    /// What is loaded in each insert slot, flat like `Settings::strip_inserts`.
    /// Remembered whether or not the load worked, so a plugin that refuses is
    /// asked once.
    inserts_loaded: Vec<Option<String>>,
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
    video: Option<TakeVideo>,
    /// Whether this take has already tried to start filming.
    ///
    /// Without it a take whose video was REFUSED — no camera, no GPU, a file
    /// that would not open — retries on every window frame, and rewrites the
    /// same error sixty times a second over whatever else the band was saying.
    ///
    video_tried: bool,
    /// A live-input tap waiting for an engine to play it.
    ///
    /// The tap is made when the INPUT opens and can only be handed to an
    /// ENGINE, and the two are opened by different edges — so it is held here
    /// rather than dropped when the second one is not there yet. See
    /// `push_monitor_settings`, which is where it gets handed over.
    pending_monitor: Option<(rtrb::Consumer<f32>, u16, [u8; crate::instrument::INPUTS], u32)>,
    /// What each input strip is called, as the picker names it. Empty for one
    /// nobody has filled.
    ///
    /// Kept here rather than read back out of the app, because the host is
    /// what knows: which inputs are open is a fact about the device and the
    /// selection, and `ivory-ui` can reach neither.
    input_names: [String; crate::instrument::INPUTS],
    /// What the LAST take is worth saying about it, if anything.
    ///
    /// **Its own field, not `engine_error`.** The video path used to put this
    /// in the audio engine's error slot, which nothing in the video path ever
    /// cleared — so "frames were dropped" sat on the owner's screen for eight
    /// minutes and across several takes, outliving the take it described by a
    /// long way. This is cleared when a take starts and by the × on the status
    /// row, and it is the last thing in the message chain because a live
    /// problem always matters more than a finished one.
    take_note: Option<String>,
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
    /// The sample rate both streams were opened with, and the system they were
    /// opened through. Same edge, same reopen, same refusal mid-take.
    ///
    /// The system is the heaviest of the three: it does not merely reopen the
    /// streams, it changes which driver stack they are opened against — so the
    /// device selected under the old one may not exist under the new one, and
    /// `reconcile_audio` will settle to "missing" and say so.
    rate_open: Option<u32>,
    system_open: Option<String>,
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
    /// The DX7 cartridge the built-in is playing out of, if one is loaded.
    ///
    /// **The host holds the voices; the UI holds the names.** Kept here rather
    /// than in `IvoryApp` because a `Voice` is a synthesizer's business and
    /// `ivory-ui` is not allowed to know what one is — the same split as the
    /// plugin picker's paths-not-modules.
    #[cfg(feature = "recorder")]
    cartridge: Option<crate::dx7::Cartridge>,
    /// The patch the editor is working on, if it is open.
    ///
    /// **A copy, not a reference into the cartridge.** Editing is not choosing:
    /// what is being built here may never be saved, and it must not modify the
    /// bank it started from. What it does do is play, immediately — see
    /// `SetPatchParam`.
    #[cfg(feature = "recorder")]
    editing: Option<crate::dx7::Voice>,
    /// The window was fullscreen when a picker was asked for, so it comes out
    /// of fullscreen, opens the picker next frame, and goes back after. See
    /// `picker_needs_windowed`.
    #[cfg(feature = "recorder")]
    refullscreen: bool,
    /// A picker deferred by one frame for that reason.
    #[cfg(feature = "recorder")]
    /// A request held back for one frame so the dialogs above it can stand
    /// down first. See `IvoryApp::native_panel_up`.
    panel_armed: bool,
    deferred_file: Option<ivory_ui::ports::FileRequest>,
    #[cfg(feature = "recorder")]
    deferred_dir: Option<ivory_ui::ports::DirRequest>,
    /// The take report already shown, so it is raised once rather than every
    /// frame until the next take.
    #[cfg(feature = "recorder")]
    reported_take: Option<String>,
    /// What the in-app browser is filtering by, so the folder it navigates
    /// into is listed the same way the first one was.
    #[cfg(feature = "recorder")]
    browse_extensions: Vec<String>,
    /// The decoded backing track, kept so the engine's `Arc` has an owner here
    /// and the file is not decoded again on every settings push.
    #[cfg(feature = "recorder")]
    track: Option<Arc<ivory_record::decode::Clip>>,
}

/// How many columns the backing track's waveform is drawn from.
///
/// **Not the pixel width of the row.** The envelope is computed once when the
/// file is imported and drawn at whatever size the window happens to be, so it
/// has to be finer than the widest band anybody will open and coarse enough to
/// stay a few kilobytes. A thousand columns is under a pixel each at 1080p.
#[cfg(feature = "recorder")]
const TRACK_WAVE_BUCKETS: usize = 1000;

/// **Every stepped row has a list to step through.**
///
/// The panel decides which rows are named-value rows ([`FxRow::step`]) and the
/// HOST decides what those names are; they meet by string key and nothing
/// makes them agree. A row whose key the host never sends draws an empty box
/// that does nothing when pressed — which is what `lpf_slope` did for exactly
/// as long as it took to write this.
///
/// [`FxRow::step`]: ivory_ui::recorder_panel::FxRow::step
#[cfg(all(test, feature = "recorder"))]
#[test]
fn the_host_offers_a_choice_for_every_stepped_row() {
    let d = effect_defaults();
    for fx in ivory_ui::recorder_panel::Fx::ALL {
        for row in fx.rows() {
            if !row.step {
                continue;
            }
            let c = d
                .choices
                .iter()
                .find(|c| c.key == row.key)
                .unwrap_or_else(|| panic!("{} has no choices for {}", fx.title(), row.key));
            assert!(!c.options.is_empty(), "{} offers an empty list", row.key);
            assert!(
                c.options.iter().any(|(k, _)| *k == c.default),
                "{}'s default {:?} is not one of its options",
                row.key,
                c.default
            );
        }
    }
    // And every sliding row has a default value, for the same reason.
    for fx in ivory_ui::recorder_panel::Fx::ALL {
        for row in fx.rows() {
            if row.step || row.key.is_empty() {
                continue;
            }
            assert!(
                d.values.contains_key(row.key),
                "{} has no shipped value for {}",
                fx.title(),
                row.key
            );
        }
    }
}

/// **What a filter knob SAYS is where the filter actually is.**
///
/// The readout is computed in `ivory-ui` from two numbers this file hands it,
/// using the same exponential the DSP uses — so the only way they can disagree
/// is if one of the two ends is wrong. That would put a confident, precise,
/// incorrect frequency under the knob, which is worse than the percentage it
/// replaced.
#[cfg(all(test, feature = "recorder"))]
#[test]
fn a_filter_knob_reads_out_where_its_filter_actually_is() {
    use ivory_ui::ports::KnobUnit;
    let d = effect_defaults();
    // The limiter reads in decibels of threshold, and the same rule applies:
    // a number under the knob that is not where the limiter actually starts
    // working is worse than no number at all.
    let (_, limiter) = d
        .units
        .iter()
        .find(|(k, _)| k == "limiter_mix")
        .expect("the limiter has no unit and would read as a percentage");
    let KnobUnit::Decibels { low, high } = *limiter else {
        panic!("the limiter does not read in decibels")
    };
    assert!(
        (low - crate::effects::LIMITER_DB.0).abs() < 1.0e-3
            && (high - crate::effects::LIMITER_DB.1).abs() < 1.0e-3,
        "the limiter advertises {low}..{high} dB and thresholds {}..{}",
        crate::effects::LIMITER_DB.0,
        crate::effects::LIMITER_DB.1
    );
    // Fully right is off, twelve o'clock is -24, fully left is -48.
    assert_eq!(
        ivory_ui::recorder_panel::knob_reading(*limiter, 1.0),
        "0.0 dB",
        "the limiter's resting position is not 0 dB"
    );
    assert_eq!(ivory_ui::recorder_panel::knob_reading(*limiter, 0.5), "-24.0 dB");
    assert_eq!(ivory_ui::recorder_panel::knob_reading(*limiter, 0.0), "-48.0 dB");

    for (key, range) in [
        ("hpf_mix", crate::effects::HPF_HZ),
        ("lpf_mix", crate::effects::LPF_HZ),
    ] {
        let (_, unit) = d
            .units
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("{key} has no unit and would read as a percentage"));
        let KnobUnit::Hertz { low, high } = *unit else {
            panic!("{key} does not read in hertz")
        };
        assert!(
            (low - range.0).abs() < 1.0e-3 && (high - range.1).abs() < 1.0e-3,
            "{key} advertises {low}..{high} Hz and sweeps {}..{} Hz",
            range.0,
            range.1
        );
        // And the readout agrees with the DSP at both ends and the middle.
        for t in [0.0_f32, 0.5, 1.0] {
            let said = ivory_ui::recorder_panel::knob_reading(*unit, t);
            let actual = range.0 * (range.1 / range.0).powf(t);
            let shown: f32 = said.split_whitespace().next().unwrap().parse().unwrap();
            let shown = if said.contains('k') { shown * 1_000.0 } else { shown };
            assert!(
                (shown - actual).abs() / actual < 0.02,
                "{key} at {t} reads {said} and the filter is at {actual:.0} Hz"
            );
        }
    }
}

/// **A directory listing is a listing, sorted and filtered.**
///
/// The in-app browser exists because `rfd` silently does nothing on a Linux
/// box with no portal and no zenity, so this is the only way to choose a file
/// there — and a browser that hid the file somebody wanted, or offered them a
/// `.DS_Store`, would be a worse answer than the silence it replaced.
#[cfg(all(test, feature = "recorder"))]
#[test]
fn the_browser_lists_folders_first_and_filters_what_it_offers() {
    let root = std::env::temp_dir().join(format!("tangent-browse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Zebra")).expect("mkdir");
    std::fs::create_dir_all(root.join("apple")).expect("mkdir");
    for f in ["b.wav", "A.MP3", "notes.txt", ".hidden.wav"] {
        std::fs::write(root.join(f), b"x").expect("write");
    }
    let exts: Vec<String> = ["wav", "mp3"].into_iter().map(str::to_owned).collect();
    let (rows, err) = DesktopApp::list_dir(&root, &exts);
    assert!(err.is_empty(), "{err}");
    let names: Vec<&str> = rows.iter().map(|e| e.name.as_str()).collect();

    // The way up, then folders, then files - each sorted without regard to
    // case, which is the order every file manager uses.
    assert_eq!(names, ["..", "apple", "Zebra", "A.MP3", "b.wav"], "{names:?}");
    assert!(rows[0].is_dir && rows[1].is_dir && rows[2].is_dir);
    assert!(!rows[3].is_dir);
    // **Case-insensitively filtered**: a file named `.MP3` is an mp3, and a
    // browser that only matched lowercase would hide half of anybody's music.
    assert!(!names.contains(&"notes.txt"), "an unmatched file was offered");
    assert!(!names.contains(&".hidden.wav"), "a dotfile was offered");

    // No filter at all offers everything but the dotfiles.
    let (all, _) = DesktopApp::list_dir(&root, &[]);
    assert!(all.iter().any(|e| e.name == "notes.txt"));
    assert!(!all.iter().any(|e| e.name.starts_with('.') && e.name != ".."));

    // A folder that is not there is a sentence, not a panic or an empty list
    // that looks like an empty folder.
    let (rows, err) = DesktopApp::list_dir(&root.join("nope"), &exts);
    assert!(rows.is_empty());
    assert!(!err.is_empty(), "a missing folder reported nothing");

    let _ = std::fs::remove_dir_all(&root);
}

/// [`effect_defaults`], for the offscreen screenshot test.
#[cfg(all(test, feature = "recorder"))]
pub fn effect_defaults_for_shot() -> ivory_ui::ports::EffectDefaults {
    effect_defaults()
}

/// What the effects ship as, for the panel that draws them.
///
/// Built from `Params::default()` rather than written out again, so the panel
/// and the audio cannot disagree about where a slider starts.
#[cfg(feature = "recorder")]
fn effect_defaults() -> ivory_ui::ports::EffectDefaults {
    use crate::effects::{Division, Params};
    let d = Params::default();
    let mut values = serde_json::Map::new();
    for (key, v) in [
        ("reverb_size", d.reverb_size),
        ("reverb_damp", d.reverb_damp),
        ("reverb_width", d.reverb_width),
        ("delay_feedback", d.delay_feedback),
        ("delay_tone", d.delay_tone),
        ("delay_width", d.delay_width),
        ("chorus_rate", d.chorus_rate),
        ("chorus_depth", d.chorus_depth),
        ("chorus_width", d.chorus_width),
        ("chorus_tone", d.chorus_tone),
        ("hpf_resonance", d.hpf_resonance),
        ("lpf_resonance", d.lpf_resonance),
        ("limiter_release", d.limiter_release),
        ("limiter_knee", d.limiter_knee),
    ] {
        values.insert(key.to_owned(), serde_json::Value::from(f64::from(v)));
    }
    ivory_ui::ports::EffectDefaults {
        values,
        // The filters read in hertz. "48%" on a corner frequency is a number
        // about the knob rather than about the sound, and the one thing
        // anybody wants to know from a filter is where it is.
        units: vec![
            (
                "hpf_mix".to_owned(),
                ivory_ui::ports::KnobUnit::Hertz {
                    low: crate::effects::HPF_HZ.0,
                    high: crate::effects::HPF_HZ.1,
                },
            ),
            (
                "lpf_mix".to_owned(),
                ivory_ui::ports::KnobUnit::Hertz {
                    low: crate::effects::LPF_HZ.0,
                    high: crate::effects::LPF_HZ.1,
                },
            ),
            // The limiter is a threshold, and a threshold has one unit.
            (
                "limiter_mix".to_owned(),
                ivory_ui::ports::KnobUnit::Decibels {
                    low: crate::effects::LIMITER_DB.0,
                    high: crate::effects::LIMITER_DB.1,
                },
            ),
        ],
        choices: vec![
            ivory_ui::ports::ChoiceParam {
                key: "delay_division".to_owned(),
                options: Division::ALL
                    .into_iter()
                    .map(|x| (x.key().to_owned(), x.label().to_owned()))
                    .collect(),
                default: d.delay_division.key().to_owned(),
            },
            slope_choice("hpf_slope", d.hpf_slope),
            slope_choice("lpf_slope", d.lpf_slope),
        ],
    }
}

/// One filter's slope, as a choice the panel can step through.
#[cfg(feature = "recorder")]
fn slope_choice(key: &str, default: crate::effects::Slope) -> ivory_ui::ports::ChoiceParam {
    ivory_ui::ports::ChoiceParam {
        key: key.to_owned(),
        options: crate::effects::Slope::ALL
            .into_iter()
            .map(|x| (x.key().to_owned(), x.label().to_owned()))
            .collect(),
        default: default.key().to_owned(),
    }
}

/// Turn the settings file's flat map into the parameters the DSP wants.
///
/// **The translation lives here, on the binary's side of the firewall.**
/// `ivory-ui` holds a map of numbers somebody moved and knows nothing about
/// comb filters; this is where a name becomes a field. A key that is missing,
/// or is the wrong kind of value, leaves that parameter at its default — which
/// is what makes a settings file written by an older build load without a
/// migration, and one written by a newer build load without an error.
#[cfg(feature = "recorder")]
fn effect_params_from(map: &serde_json::Map<String, serde_json::Value>) -> crate::effects::Params {
    use crate::effects::{Division, Params};
    let mut p = Params::default();
    let num = |key: &str| map.get(key).and_then(serde_json::Value::as_f64).map(|v| v as f32);
    for (key, dst) in [
        ("reverb_size", &mut p.reverb_size),
        ("reverb_damp", &mut p.reverb_damp),
        ("reverb_width", &mut p.reverb_width),
        ("delay_feedback", &mut p.delay_feedback),
        ("delay_tone", &mut p.delay_tone),
        ("delay_width", &mut p.delay_width),
        ("chorus_rate", &mut p.chorus_rate),
        ("chorus_depth", &mut p.chorus_depth),
        ("chorus_width", &mut p.chorus_width),
        ("chorus_tone", &mut p.chorus_tone),
        ("hpf_resonance", &mut p.hpf_resonance),
        ("lpf_resonance", &mut p.lpf_resonance),
        ("limiter_release", &mut p.limiter_release),
        ("limiter_knee", &mut p.limiter_knee),
    ] {
        if let Some(v) = num(key) {
            *dst = v;
        }
    }
    if let Some(d) = map
        .get("delay_division")
        .and_then(serde_json::Value::as_str)
        .and_then(Division::from_key)
    {
        p.delay_division = d;
    }
    for (key, dst) in [
        ("hpf_slope", &mut p.hpf_slope),
        ("lpf_slope", &mut p.lpf_slope),
    ] {
        if let Some(v) = map
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(crate::effects::Slope::from_key)
        {
            *dst = v;
        }
    }
    p.sane()
}

/// A parsed cartridge as the picker wants it: names and a bank, no voices.
#[cfg(feature = "recorder")]
fn cartridge_info(
    cart: &crate::dx7::Cartridge,
    error: &str,
    factory: bool,
) -> ivory_ui::ports::CartridgeInfo {
    ivory_ui::ports::CartridgeInfo {
        bank: cart.name.clone(),
        bad_checksum: !cart.checksum_ok,
        voices: cart.names(),
        error: error.to_owned(),
        factory,
    }
}

/// How often free space is measured again while the band is open.
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

        // **Who is looking at the camera.**
        //
        // The conversion is the cost, and the capture thread was doing thirty
        // a second for as long as the band was open, whether or not anything
        // was looking. Measured on a 2013 MacBook Air: 35.6% of a core, idle,
        // with the pane hidden and no take rolling, on a machine that then
        // could not keep up with a take.
        //
        // **Three states and no numbers**, which is the fix for what the first
        // version of this got wrong. It sent a fixed ten a second for a
        // preview, on every host — and that number came from the Air's JPEG
        // decode. Only V4L2 does a JPEG decode; on macOS a conversion is a
        // BGRA-to-RGBA copy costing a fraction of a millisecond, so the cap
        // bought nothing and threw away two thirds of the preview's
        // smoothness. What a preview can afford is now measured where the
        // conversions happen. See `camera::FrameWant`.
        use ivory_record::camera::FrameWant;
        self.recorder.session.set_camera_want(
            if self.recorder.session.is_recording() && self.app.export_spec().video.wants_video() {
                FrameWant::Every
            } else if self.app.recorder_band_open() || self.app.camera_pane_showing() {
                FrameWant::Preview
            } else {
                FrameWant::None
            },
        );

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
            //
            // **Moved, not cloned.** This was `frame.pixels.clone()`: a full
            // 3.7 MB copy per frame at 720p, on the UI thread, of pixels that
            // were about to be dropped anyway. And the buffer it displaces
            // goes BACK to the capture thread, which is what makes the
            // steady state allocation-free — see `FrameSlot::recycle`.
            let spent = self
                .recorder
                .camera_rgba
                .replace((frame.pixels, frame.width, frame.height));
            if let Some((pixels, ..)) = spent {
                self.recorder.session.recycle_frame(pixels);
            }
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
            // Only while something is actually being monitored: a ring nobody
            // is listening to is not latency, it is a ring.
            monitor_ms: self
                .recorder
                .engine
                .as_ref()
                .filter(|_| self.app.input_monitor())
                .map(crate::instrument::Engine::monitor_backlog_ms),
        });

        // The buffer size, on the edge. Reopening both streams is expensive —
        // the output takes every instrument with it — so it happens when the
        // choice CHANGES and not while a take is rolling.
        // A monitor that would not open a moment ago. See `engine_retry`.
        if self.recorder.engine.is_none() && self.app.recorder_band_open() {
            if let Some((at, _)) = self.recorder.engine_retry {
                if std::time::Instant::now() >= at {
                    self.open_audio_path(ctx);
                }
            }
        }

        // Whether a multichannel interface lists its inputs one by one. Pushed
        // every frame rather than on an edge: it is one atomic store, and the
        // list it changes is built fresh every time the picker opens, so there
        // is no state to reconcile and nothing to reopen.

        let want_buffer = self.app.buffer_frames();
        let want_rate = self.app.sample_rate();
        let want_system = self.app.audio_system();
        let path_changed = self.recorder.buffer_open != want_buffer
            || self.recorder.rate_open != want_rate
            || self.recorder.system_open != want_system;
        if path_changed && !self.recorder.session.is_recording() {
            self.recorder.buffer_open = want_buffer;
            self.recorder.rate_open = want_rate;
            self.recorder.system_open = want_system.clone();
            // The system FIRST, and before anything is opened: every device
            // lookup in `ivory_record::audio` goes through it, so setting it
            // after the streams were built would open them on the old stack and
            // then list devices from the new one.
            ivory_record::audio::set_system(want_system.as_deref());
            // Dropping the engine stops the output stream, and STARTING ONE
            // AGAIN is not optional. `start_engine` otherwise runs only on the
            // edge of the band opening, so dropping it here left the app with
            // no monitor and no instrument until the band was closed and
            // reopened — which is not a thing anybody would think to try.
            //
            // The old stream's `Drop` releases the device before the new one
            // asks for it, which is the ordering CoreAudio needs. The
            // five-second instrument load still happens over the following
            // frames, announced as usual.
            self.recorder.engine = None;
            self.recorder.plugin_loaded = std::array::from_fn(|_| None);
            self.open_audio_path(ctx);
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
                        "loading {} - instruments warm up for a few seconds so \
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
                "starting the camera - this can take a few seconds on a USB \
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
                    // Where the fix lives is not the same sentence on every
                    // platform, and naming the wrong panel is worse than
                    // naming none.
                    if cfg!(target_os = "macos") {
                        "the camera is open but sending no picture - check \
                         Camera access in System Settings > Privacy & Security"
                    } else if cfg!(target_os = "linux") {
                        "the camera is open but sending no picture - another \
                         program may be holding it, or the format may not be \
                         supported"
                    } else {
                        "the camera is open but sending no picture"
                    }
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
            // **And what this take is going to be missing.**
            //
            // Mute is the only thing that decides what a take is made of now,
            // which is one rule where there used to be four — but a silent
            // rule that costs somebody a take is no better than the setting it
            // replaced. So a muted source with something in it says so, once,
            // while there is still a take to save.
            //
            // Only while one is RUNNING. Muting the microphone to practise is
            // a normal thing to do and a band that complained about it all
            // afternoon would teach everybody to ignore the line that also
            // says the camera was denied.
            .or_else(|| self.muted_out_of_the_take())
            // And what the LAST take was worth saying, which is the only thing
            // here that is about the past — so it goes last.
            .or_else(|| self.recorder.take_note.clone())
            // **The take's report is not here any more.** It was the last
            // `or_else` in this chain, which meant it sat in a one-line strip
            // competing with live errors and stayed up until something else
            // replaced it. It is a dialog now — see `report_take` below and
            // `Dialog::TakeSummary`. What remains in this chain is the live
            // problems, which belong in the band because they are true NOW.
            ;

        let preview = self.recorder.preview.as_ref().map(|h| ivory_ui::recorder::Preview {
            texture: h.id(),
            size: self.recorder.preview_px,
        });
        // Computed BEFORE the mutable borrow of the app: `chosen_plugin()`
        // borrows it immutably and `recorder_state_mut()` holds it mutably.
        let engine = self.recorder.engine.as_ref();
        let slots: [ivory_ui::recorder::SlotState; ivory_ui::recorder::SLOTS] =
            std::array::from_fn(|i| {
                // **The built-in is never "missing".** It is compiled in, so
                // there is no `plugin(i)` behind it and every test below would
                // otherwise conclude that the instrument failed to load — which
                // is what the band said, in red, about the one instrument that
                // cannot fail.
                if self.app.chosen_plugin(i) == Some(ivory_ui::dialogs::BUILTIN_PATH) {
                    return ivory_ui::recorder::SlotState {
                        // The PATCH, not the instrument. "Tangent DX7" in a
                        // slot says what it is; "E.PIANO 1" says what it
                        // sounds like, which is the thing being chosen.
                        name: Some(self.builtin_patch_name()),
                        missing: false,
                        // Its editor is the patch picker, and the app opens
                        // that itself — but the row has to offer the gesture.
                        has_editor: true,
                        editor_open: matches!(
                            self.app.open_dialog(),
                            Some(ivory_ui::dialogs::Dialog::PatchPicker { slot, .. }) if *slot == i
                        ),
                    };
                }
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

        // **Once per take, on the edge.** `last_summary` keeps answering with
        // the same take until the next one, so the message it carries is the
        // trigger: folders are timestamped, so two takes never produce the
        // same one. Before the state borrow, because raising a dialog is
        // another `&mut self.app`.
        if let Some(summary) = self.recorder.session.last_summary() {
            let (message, problem) = (summary.message(), summary.is_problem());
            let silent = summary.is_silent();
            if self.reported_take.as_deref() != Some(message.as_str()) {
                self.reported_take = Some(message.clone());
                // **A silent take gets a line, not a modal.** It is worth
                // saying — "I recorded silence" is the failure this recorder
                // exists to prevent — and it is not worth a window that
                // swallows every event until it is found and closed. See
                // `Summary::is_problem`.
                if silent {
                    self.recorder.take_note = Some(
                        "that take is silent - check the input, and the mute \
                         buttons in the mixer"
                            .to_owned(),
                    );
                }
                self.app.report_take(message, problem);
            }
        }
        // **The alias replaces the interface's name, once, for all of it.**
        // Read BEFORE the state borrow, like everything else here: the app is
        // borrowed mutably for the rest of this function.
        let alias = self.app.input_alias().trim().to_owned();
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
        // The output, which is a different signal: the VU shows what is being
        // recorded and this shows what leaves. On a machine with an interface
        // plugged in those are not the same thing at all.
        if let Some(e) = self.recorder.engine.as_ref() {
            state.master = e.meters();
            state.gr_db = e.gain_reduction_db();
            // The desk's own meters. Read-and-reset on the engine's side, so
            // this has to happen exactly once a frame and here is that once.
            state.strip_peaks = e.strip_peaks();
            // **And the VU falls back to it when nothing else is feeding.**
            // The band's meter shows what is being recorded: the input when
            // there is one, and otherwise the instrument. With no interface
            // selected neither of the session's two sources exists, so it
            // answered SILENT — a dead needle and a clip lamp that could never
            // light, on a Mac with a piano plugged into it and the FM playing.
            // Read ONCE, because the peaks clear on read: the same numbers go
            // to both meters rather than one of them getting zero.
            if !self.recorder.session.has_meter_source() {
                state.meters = state.master;
            }
            // **And the lamp means "something clipped", whatever the needle is
            // showing.** With an input selected the VU meters the INPUT, so a
            // built-in FM driven into the ceiling clipped the output and lit
            // nothing at all — "choose a mic and clipping is not possible",
            // on both platforms, for the same reason. The needle still answers
            // one question; the lamp beside it was always the other one.
            state.meters.clipped |= state.master.clipped;
        } else {
            state.master = ivory_ui::recorder::Meters::SILENT;
            state.gr_db = 0.0;
        }
        // **Levels with nothing plugged in, for looking at the desk.**
        //
        // Every meter here comes from the engine, so a machine with no
        // interface and no instrument shows twelve strips of silence — and a
        // meter, a scale and a gain-reduction bar are exactly the things that
        // have to be LOOKED at rather than reasoned about. Pushed after the
        // engine's own numbers, because it is overriding them.
        //   IVORY_DEMO_LEVELS=1 /Applications/Tangent.app/Contents/MacOS/tangent
        if std::env::var("IVORY_DEMO_LEVELS").is_ok() {
            for (i, peaks) in state.strip_peaks.iter_mut().enumerate() {
                // Different on every channel and different on the two sides of
                // each, so a bar drawn from the wrong lane is visible.
                let l = 0.12 + 0.13 * (i % 6) as f32;
                *peaks = [l, l * 0.55];
            }
            state.gr_db = 7.5;
            state.master = ivory_ui::recorder::Meters {
                left: ivory_ui::recorder::Level { peak: 0.79, rms: 0.5, hold: 0.9 },
                right: ivory_ui::recorder::Level { peak: 0.44, rms: 0.3, hold: 0.55 },
                mono: false,
                clipped: false,
            };
        }
        state.dest = shorten_home(&root);
        state.folder_preview = take::folder_name(
            &take::WallTime::now_utc(),
            name.as_deref().and_then(take::sanitise_slug).as_deref(),
        );
        state.audio_name = open_name.or(audio_uid);
        // **One strip per input, filled from the selection.** The names and
        // the widths come from the same function the picks do, so the column
        // the mixer draws and the channels the capture keeps cannot drift
        // apart — see `devices::open_inputs`.
        let open_inputs = crate::devices::open_inputs(&self.recorder.audio);
        // **The alias replaces the interface's name, once, for all of it.**
        // "Scarlett 18i20 USB  -  input 3" does not fit a mixer strip; "x - 3"
        // does, and the half worth shortening is the one that is the same on
        // every channel of the box.
        for i in 0..ivory_ui::recorder::INPUTS {
            let (device, channel, stereo) = open_inputs
                .get(i)
                .cloned()
                .unwrap_or_else(|| (String::new(), String::new(), false));
            let label = if device.is_empty() {
                String::new()
            } else {
                let head = if alias.is_empty() { device.as_str() } else { alias.as_str() };
                if channel.is_empty() {
                    head.to_owned()
                } else {
                    format!("{head} - {channel}")
                }
            };
            self.recorder.input_names[i].clone_from(&label);
            state.inputs[i] = ivory_ui::recorder::InputState { label, stereo };
        }
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
            && matches!(
                std::env::var("IVORY_OPEN_EDITOR").as_deref(),
                Ok("1") | Ok("patch")
            )
        {
            let due = self
                .recorder
                .dev_editor_at
                .get_or_insert_with(|| std::time::Instant::now() + DEV_EDITOR_DELAY);
            if std::time::Instant::now() >= *due {
                self.recorder.dev_editor_done = true;
                // Through the app's own gesture and not `Engine::open_editor`,
                // so the hook opens whatever a CLICK would open — the built-in's
                // patch picker as readily as a VST3's window. A hook that called
                // the engine directly could only ever reproduce half the bugs.
                self.app.open_slot_editor(0);
                // `=patch` goes one further, to the patch EDITOR, which is
                // otherwise two clicks in and cannot be reached from a script.
                if std::env::var("IVORY_OPEN_EDITOR").as_deref() == Ok("patch") {
                    self.app.apply_dialog_action(ivory_ui::dialogs::DialogAction::EditPatch {
                        slot: 0,
                    });
                }
                eprintln!("IVORY_OPEN_EDITOR: opened slot 1");
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
                self.open_audio_path(ctx);
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

        // The in-app browser's three messages, pumped out here with the native
        // dialogs because listing a directory is disk I/O and `ivory-ui` does
        // not do that.
        if let Some(dir) = self.app.take_browse_request() {
            let exts = self.browse_extensions.clone();
            let (entries, error) = Self::list_dir(&dir, &exts);
            self.app.set_browser(dir, entries, error);
        }
        if let Some((purpose, file)) = self.app.take_browsed_file() {
            self.finish_file_choice(purpose, &file);
        }
        if let Some((purpose, dir)) = self.app.take_browsed_dir() {
            self.finish_dir_choice(purpose, dir);
        }

        if let Some(request) = self.app.take_directory_request().or_else(|| self.deferred_dir.take())
        {
            if Self::picker_needs_windowed(ctx) {
                self.deferred_dir = Some(request);
                self.refullscreen = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                return;
            }
            // One frame for the dialogs to stop floating, as for files.
            if self.panel_armed {
                self.panel_armed = false;
            } else if self.app.native_panel_up() {
                self.panel_armed = true;
                self.deferred_dir = Some(request);
                ctx.request_repaint();
                return;
            }
            // No portal and no zenity: our own browser, as for files.
            if !Self::native_dialogs_work() {
                let at = request
                    .start_at
                    .filter(|p| p.is_dir())
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| std::path::PathBuf::from("/"));
                self.browse_extensions = Vec::new();
                let (entries, _) = Self::list_dir(&at, &[]);
                self.app.native_panel_closed();
                self.app.open_browser(
                    request.title.clone(),
                    ivory_ui::dialogs::BrowseFor::Folder(request.purpose),
                    at,
                    entries,
                );
                return;
            }
            // Parented, for the reason `take_cartridge_request` gives: an
            // unparented panel can open behind the app on Windows.
            let mut dialog = rfd::FileDialog::new()
                .set_title(&request.title)
                .set_parent(frame);
            if let Some(start) = request.start_at.filter(|p| p.exists()) {
                dialog = dialog.set_directory(start);
            }
            let picked = dialog.pick_folder();
            self.app.native_panel_closed();
            if let Some(dir) = picked {
                self.finish_dir_choice(request.purpose, dir);
            }
            self.restore_fullscreen(ctx);
        }
        self.take_cartridge_request(ctx, frame);
        // Scanning reads directories, so it belongs out here with the other
        // things the UI is not allowed to do — and it runs AFTER the folder
        // picker above, so a folder added this frame is in the list this frame
        // rather than one frame later.
        if self.app.take_plugin_rescan() {
            let extra = self.app.plugin_folders();
            self.app.set_plugin_list(ivory_host::discover_in(&extra));
        }
        // **The user's effect, across the effects bus.**
        //
        // Reconciled here with the plugin rack and for the same reasons:
        // `Module::open` runs somebody else's initialiser and `Instance::create`
        // can take seconds, so it happens after a frame and never inside one.
        // Once per change, because `bus_effect_loaded` remembers what was asked
        // for whether or not it worked — a plugin that will not load must not
        // be retried sixty times a second.
        // **ONE per call, like the instrument rack**, and for the same reason:
        // loading blocks for seconds, and thirty-nine slots reconciled in one
        // frame would be a window that stopped for a minute. Each is
        // remembered whether or not it worked, so a plugin that will not load
        // is not retried sixty times a second.
        let inserts = ivory_ui::recorder::INSERTS;
        let want_at = |i: usize| {
            self.app
                .insert(i / inserts, i % inserts)
                .map(str::to_owned)
        };
        if let Some(i) = (0..self.recorder.inserts_loaded.len())
            .find(|i| want_at(*i) != self.recorder.inserts_loaded[*i])
        {
            let want = want_at(i);
            self.recorder.inserts_loaded[i] = want.clone();
            if let Some(e) = self.recorder.engine.as_mut() {
                let path = want.as_ref().map(std::path::Path::new);
                match e.load_insert(i / inserts, i % inserts, path) {
                    Ok(_) => self.recorder.engine_error = None,
                    Err(err) => {
                        let which = want
                            .as_deref()
                            .and_then(|p| p.rsplit('/').next())
                            .unwrap_or("that plugin");
                        let channel = ivory_ui::recorder::Strip::all()
                            .get(i / inserts)
                            .map_or_else(|| "the master".to_owned(), |s| s.label());
                        self.recorder.engine_error =
                            Some(format!("{which} did not load on {channel}: {err}"));
                    }
                }
            }
        }

        // **The shipped bank, put back.** The same call a first launch makes,
        // which is the point: there is one definition of "the cartridge that
        // ships" and both paths read it.
        if self.app.take_factory_cartridge() {
            let cart = crate::dx7::factory();
            self.app.set_cartridge(cartridge_info(&cart, "", true));
            // **And the sound changes now.** Refilling the list without
            // changing what is playing is a button that appears to do nothing,
            // which is the complaint this answers. Patch 0, because the
            // setting was reset to it.
            if let Some(v) = cart.voices.first().copied() {
                if let Some(e) = self.recorder.engine.as_mut() {
                    e.set_builtin_voice(v);
                }
            }
            self.cartridge = Some(cart);
        }

        // The take's video, on the EDGES of the session's own state. Placed
        // after the request loop so that a Record pressed this frame is already
        // rolling by the time this asks, and a Stop pressed this frame is
        // already stopped — the alternative is a video that starts and ends one
        // window frame late at both ends.
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

        // The × on the status row. The message is the host's, so the host is
        // what puts it away — see `IvoryApp::take_dismiss_message`.
        if self.app.take_dismiss_message() {
            self.recorder.take_note = None;
            // **And the engine's error, which is what is usually showing.**
            // "Pro-R 2 is an effect, not an instrument" is a correct refusal
            // and a finished conversation — the user has read it and there is
            // nothing to do — but nothing in that path ever cleared it, so it
            // sat there with an × beside it that did not apply to it. An ×
            // that dismisses one message and not the one under the cursor is
            // worse than no × at all.
            self.recorder.engine_error = None;
        }
        while let Some(request) = self.app.take_recorder_request() {
            match request {
                R::Toggle => {
                    // **Whatever the last take had to say, it is not about
                    // this one.** Stale post-take advice that outlives the
                    // take it describes is how a line about dropped frames sat
                    // on screen for eight minutes across several takes.
                    self.recorder.take_note = None;
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
                // **Every latch, or the light does not go out.** The warning
                // is an OR across the input tracker, the take summary and the
                // instrument bus's own — and the VU paints itself red from the
                // first of those, so a dismiss that missed one would look like
                // a button that does nothing.
                R::DismissClip => {
                    self.recorder.session.clear_clip();
                    if let Some(e) = self.recorder.engine.as_ref() {
                        e.clear_clip();
                    }
                }
                R::Audition { notes, on } => {
                    // **All of them at one instant.** The events are stamped
                    // once and pushed together, so a four-note chord arrives as
                    // a chord. They used to be sent as on/off pairs per note,
                    // which put a far-future note-off second in the queue,
                    // where the drain parked it as "not due yet" and stopped —
                    // delivering the rest of the chord when the first note
                    // ended rather than when it started.
                    //
                    // Through `send_midi`, so an auditioned note is
                    // indistinguishable from a played one: same path, same
                    // instrument, and it lands in a take that is rolling, which
                    // is what somebody demonstrating a voicing on camera wants.
                    let sent: Option<(i64, Vec<[u8; 3]>)> =
                        self.recorder.engine.as_ref().map(|e| {
                            let at = e.timebase().now();
                            let status = if on { 0x90 } else { 0x80 };
                            let vel = if on {
                                ivory_ui::recorder::AUDITION_VELOCITY
                            } else {
                                64
                            };
                            let events: Vec<[u8; 3]> =
                                notes.iter().map(|n| [status, *n, vel]).collect();
                            for ev in &events {
                                e.send_midi(at, ev);
                            }
                            (at, events)
                        });
                    // **And into the take's `.mid`.** The audio always carried
                    // these; the file never did, so a take of somebody
                    // demonstrating a voicing had the sound and no notes.
                    // Captured with the stamp the engine was given, so the two
                    // cannot drift.
                    if let Some((at, events)) = sent {
                        for ev in events {
                            self.recorder.session.capture_app_midi(at, ev);
                        }
                    }
                }
                R::EditPatch { slot: _ } => {
                    // Whatever is playing, which is what somebody means by
                    // "edit this patch". A fresh install starts from the
                    // default, which is a patch rather than silence.
                    let v = self.editing.unwrap_or_else(|| self.current_voice());
                    self.editing = Some(v);
                    self.push_patch_edit(None);
                }
                R::SetPatchParam {
                    group,
                    index,
                    value,
                } => {
                    if let Some(v) = self.editing.as_mut() {
                        crate::dx7::edit::apply(v, group, index, value);
                        let v = *v;
                        // **Heard as it is turned.** An editor that only
                        // applied on OK makes every change a guess.
                        if let Some(e) = self.recorder.engine.as_mut() {
                            e.set_builtin_voice(v);
                        }
                        self.push_patch_edit(None);
                    }
                }
                R::SetPatchName(name) => {
                    if let Some(v) = self.editing.as_mut() {
                        v.set_name(&name);
                        self.push_patch_edit(None);
                    }
                }
                R::SavePatch => {
                    let said = self.save_patch();
                    self.push_patch_edit(Some(said));
                }
                R::ChoosePatch { slot: _, index } => {
                    // `usize::MAX` is the built-in row. Anything else indexes
                    // the cartridge, and a stale index from a settings file
                    // written against a cartridge that has since been replaced
                    // simply finds nothing and leaves the sound alone.
                    let voice = if index == usize::MAX {
                        Some(crate::dx7::Voice::default())
                    } else {
                        self.cartridge
                            .as_ref()
                            .and_then(|c| c.voices.get(index))
                            .copied()
                    };
                    if let (Some(v), Some(e)) = (voice, self.recorder.engine.as_mut()) {
                        e.set_builtin_voice(v);
                    }
                }
                // An insert's own window. Same one-row-two-names bargain as an
                // instrument's: press it again and the window that is up goes
                // away.
                R::OpenInsertEditor(strip, slot) => {
                    if let Some(e) = self.recorder.engine.as_mut() {
                        if e.insert_editor_open(strip, slot) {
                            e.close_insert_editor(strip, slot);
                        } else if let Err(err) = e.open_insert_editor(strip, slot) {
                            self.recorder.engine_error =
                                Some(format!("could not open the effect's window: {err}"));
                        }
                    }
                }
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

    /// The patch the settings point at, if the cartridge has one there.
    ///
    /// `None` rather than a default, because the caller is deciding whether to
    /// SEND anything: a patch being edited must not be replaced by the one the
    /// settings remember every time a device is reopened.
    fn chosen_voice(&self) -> Option<crate::dx7::Voice> {
        if self.editing.is_some() {
            return None;
        }
        let (_, patch) = self.app.dx7_choice();
        self.cartridge.as_ref()?.voices.get(patch).copied()
    }

    /// The patch the built-in is playing right now.
    fn current_voice(&self) -> crate::dx7::Voice {
        let (_, patch) = self.app.dx7_choice();
        self.cartridge
            .as_ref()
            .and_then(|c| c.voices.get(patch))
            .copied()
            .unwrap_or_default()
    }

    /// Where a patch made here is written.
    ///
    /// **Beside the settings, not in them.** It is a SysEx bank: an ordinary
    /// one, which Dexed and a real DX7 will both open. A patch editor whose
    /// work could only be read back by this app would be a worse deal than the
    /// hardware offered in 1983.
    fn user_bank_path() -> std::path::PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        home.join(".config").join("ivory").join("my-patches.syx")
    }

    /// Hand the editor what it is editing.
    fn push_patch_edit(&mut self, note: Option<String>) {
        let Some(v) = self.editing else { return };
        let path = Self::user_bank_path();
        let edit = crate::dx7::edit::to_edit(&v, &path.to_string_lossy());
        self.app.set_patch_edit(edit, note);
    }

    /// Write the patch being edited into the user's own bank.
    ///
    /// **Appended, never overwritten.** A save that replaced the last one
    /// would lose the version somebody was happy with an hour ago, and a bank
    /// holds thirty-two: the oldest is dropped only once it is truly full, and
    /// the message says so.
    fn save_patch(&mut self) -> String {
        let Some(v) = self.editing else {
            return String::new();
        };
        let path = Self::user_bank_path();
        let mut voices: Vec<crate::dx7::Voice> = crate::dx7::Cartridge::load(&path)
            .map(|c| c.voices)
            .unwrap_or_default();
        // Everything after the last patch anybody saved is the padding this
        // wrote last time. Trimming it is what makes "append" mean append.
        while voices.last() == Some(&crate::dx7::Voice::default()) {
            voices.pop();
        }
        let full = voices.len() >= 32;
        if full {
            voices.remove(0);
        }
        voices.push(v);
        let cart = crate::dx7::Cartridge::of("my-patches", voices);
        match cart.save(&path) {
            Ok(()) => {
                let where_to = path
                    .file_name()
                    .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                if full {
                    format!("Saved to {where_to}. The bank was full, so the oldest went.")
                } else {
                    format!("Saved \"{}\" to {where_to}.", v.display_name())
                }
            }
            Err(e) => format!("Could not save: {e}"),
        }
    }

    /// What the built-in is playing, for its slot row.
    ///
    /// The patch's own name when a cartridge is loaded, and the instrument's
    /// otherwise — a fresh install has no cartridge and "E.PIANO 1" alone would
    /// look like a VST3 nobody remembers installing.
    fn builtin_patch_name(&self) -> String {
        let (_, patch) = self.app.dx7_choice();
        self.cartridge
            .as_ref()
            .and_then(|c| c.voices.get(patch))
            .map(crate::dx7::Voice::display_name)
            .unwrap_or_else(|| ivory_ui::dialogs::BUILTIN_NAME.to_owned())
    }

    /// Read the cartridge named in the settings, if it is still there.
    ///
    /// Failure is not reported: see the call site. The built-in patch always
    /// sounds, so the worst case is the sound somebody would have had on a
    /// fresh install.
    fn load_cartridge_at_launch(&mut self) {
        let (path, patch) = self.app.dx7_choice();
        let (path, patch) = (path.to_owned(), patch);
        // **No cartridge chosen means the one that ships**, not silence and not
        // a single patch. Sixteen electric pianos and sixteen jazz guitars are
        // in the binary; a fresh install has them all without opening a file
        // dialog, and "Load .syx..." is for somebody who wants somebody else's.
        let mut is_factory = path.is_empty();
        let cart = if path.is_empty() {
            crate::dx7::factory()
        } else {
            // A path that has moved falls back to the shipped bank rather than
            // to nothing: cartridges live in sample folders that get
            // reorganised, and an app that goes silent because a file moved is
            // an app that looks broken.
            crate::dx7::Cartridge::load(std::path::Path::new(&path)).unwrap_or_else(|_| {
                // The remembered file has gone. What is playing IS the shipped
                // bank now, and the picker must say so rather than offering to
                // go back to the bank it is already on.
                is_factory = true;
                crate::dx7::factory()
            })
        };
        self.app.set_cartridge(cartridge_info(&cart, "", is_factory));
        if let Some(v) = cart.voices.get(patch).copied() {
            if let Some(e) = self.recorder.engine.as_mut() {
                e.set_builtin_voice(v);
            }
        }
        self.cartridge = Some(cart);
    }

    /// Raise the file panel for a cartridge and load what comes back.
    ///
    /// Out here with the folder picker and for the same reason: `rfd` runs a
    /// nested run loop, and raising one inside an egui frame re-enters the
    /// frame already on the stack.
    fn take_cartridge_request(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(request) = self
            .app
            .take_file_request()
            .or_else(|| self.deferred_file.take())
        else {
            self.restore_fullscreen(ctx);
            return;
        };
        // Out of fullscreen first, and open on the frame after: see
        // `picker_needs_windowed`. This is the case that froze the app.
        if Self::picker_needs_windowed(ctx) {
            self.deferred_file = Some(request);
            self.refullscreen = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            return;
        }
        // **And one frame for the dialogs to stop floating.**
        //
        // A dialog is always-on-top so that a modal one cannot end up buried
        // behind the main window, which is an app that has silently frozen.
        // The OS's file panel is parented to the MAIN window, so it opens
        // UNDERNEATH any such dialog — which is what "load sysex opens behind
        // the instrument window" is. `native_panel_up` takes the dialogs down
        // to a normal level, but a window level is a property of the surface
        // and therefore changes on the NEXT frame; opening the panel in the
        // same frame that asked for it would race that change and lose.
        if self.arm_panel(ctx, &request) {
            return;
        }
        let purpose = request.purpose;
        // **No portal, no zenity: our own browser.** Otherwise this opens a
        // dialog that never appears and returns the same `None` a cancel does.
        if !Self::native_dialogs_work() {
            let at = request
                .start_at
                .filter(|p| p.is_dir())
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| std::path::PathBuf::from("/"));
            let (entries, _) = Self::list_dir(&at, &request.extensions);
            self.browse_extensions = request.extensions.clone();
            // Our own browser is a dialog like any other, and wants to float.
            self.app.native_panel_closed();
            self.app.open_browser(
                request.title.clone(),
                ivory_ui::dialogs::BrowseFor::File(request.purpose),
                at,
                entries,
            );
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title(&request.title);
        // **Parented to the window that asked for it.**
        //
        // Without this the panel is a top-level window of its own, and on
        // Windows it can open BEHIND the app — the button appears to do
        // nothing, which is exactly what a tester reports as "I cannot load
        // any file". macOS puts an unparented panel in front regardless, which
        // is why this was invisible here.
        dialog = dialog.set_parent(frame);
        if let Some(start) = request.start_at.filter(|p| p.exists()) {
            dialog = dialog.set_directory(start);
        }
        if !request.extensions.is_empty() {
            let exts: Vec<&str> = request.extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(&request.extension_label, &exts);
            // **And everything else, as a second filter.**
            //
            // A filter DIMS non-matching files on macOS and HIDES them on
            // Windows. Cartridges in the wild are named `.syx`, `.SYX`, `.dx7`,
            // `.bin` and very often nothing at all — a folder that looks empty
            // is a dialog somebody closes again, and they are right to.
            dialog = dialog.add_filter("All files", &["*"]);
        }
        let file = dialog.pick_file();
        // **Whether or not anything was chosen**, on both counts: back to
        // fullscreen, because a cancel must not leave the window in a shape
        // the user did not ask for, and dialogs may float again, because a
        // cancelled panel is a panel that has gone.
        self.app.native_panel_closed();
        self.restore_fullscreen(ctx);
        let Some(file) = file else { return };
        self.finish_file_choice(purpose, &file);
    }

    /// Reload last session's backing track, if the file is still there.
    ///
    /// **Quietly when it is not.** A file that has been moved or is on a drive
    /// that is not plugged in is the ordinary case for a path remembered
    /// across launches; an error dialog on startup about a backing track
    /// nobody has asked for yet is not.
    #[cfg(feature = "recorder")]
    fn load_track_at_launch(&mut self) {
        let (path, from, to) = self.app.track_settings();
        if path.is_empty() {
            return;
        }
        let path = std::path::PathBuf::from(path);
        if !path.is_file() {
            return;
        }
        self.load_track(&path);
        // The trim is put BACK, because it belongs to this file — which is the
        // one being reloaded. `set_track_path` clears it, on the reasoning
        // that a NEW file should not open already cut to the last one's
        // length, and that reasoning does not apply here.
        self.app.set_track_trim(from, to);
    }

    /// What to do with a chosen folder, whichever picker chose it.
    #[cfg(feature = "recorder")]
    fn finish_dir_choice(&mut self, purpose: ivory_ui::ports::DirPurpose, dir: std::path::PathBuf) {
        match purpose {
            ivory_ui::ports::DirPurpose::RecordRoot => self.app.set_record_dir(dir),
            ivory_ui::ports::DirPurpose::PluginFolder => self.app.add_plugin_folder(dir),
        }
    }

    /// What to do with a chosen file, whichever picker chose it.
    #[cfg(feature = "recorder")]
    fn finish_file_choice(
        &mut self,
        purpose: ivory_ui::ports::FilePurpose,
        file: &std::path::Path,
    ) {
        if purpose == ivory_ui::ports::FilePurpose::BackingTrack {
            self.load_track(file);
            return;
        }
        match crate::dx7::Cartridge::load(file) {
            Ok(cart) => {
                self.app.set_cartridge(cartridge_info(&cart, "", false));
                self.app
                    .set_dx7_cartridge(file.to_string_lossy().into_owned());
                self.cartridge = Some(cart);
            }
            // Reported INTO the dialog rather than as a message box, because
            // the dialog is still open and the next thing the user will do is
            // pick a different file from the same folder.
            Err(e) => self
                .app
                .set_cartridge(ivory_ui::ports::CartridgeInfo {
                    error: e,
                    ..Default::default()
                }),
        }
    }

    /// Whether a picker has to wait for the window to leave fullscreen first.
    ///
    /// **Linux only, and it is not a hang.** Under i3 — and under any X11 WM
    /// that honours `_NET_WM_STATE_FULLSCREEN` properly — a fullscreen window
    /// sits above everything, including a modal file panel. `rfd::pick_file`
    /// is a BLOCKING call: it stops the main thread until the panel returns.
    /// Put those together and choosing a backing track from fullscreen froze
    /// the whole app with nothing in the console, because the app was waiting
    /// on a dialog that had opened underneath the window and could not be
    /// seen, focused or dismissed.
    ///
    /// So the window comes out of fullscreen first, the picker opens on the
    /// frame after — the change needs a frame to reach the WM — and fullscreen
    /// goes back when the picker is done. macOS and Windows put a panel in
    /// front of a fullscreen window themselves and are left alone.
    #[cfg(target_os = "linux")]
    fn picker_needs_windowed(ctx: &egui::Context) -> bool {
        ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
    }

    #[cfg(not(target_os = "linux"))]
    fn picker_needs_windowed(_ctx: &egui::Context) -> bool {
        false
    }

    /// Put fullscreen back after a picker that had to leave it — **and not
    /// while one is still open.**
    ///
    /// This is what 4.18.0 got wrong, and it turned a bad bug into a worse
    /// one. On a box with no portal the picker is our own `Dialog::FileBrowser`,
    /// which is a CHILD VIEWPORT — a second window. The sequence was: leave
    /// fullscreen, open the browser next frame, and then, on the frame after
    /// that, find no pending request and put fullscreen straight back. Under
    /// i3 a fullscreen window sits above everything, so the browser was buried
    /// the instant it appeared — and `app.rs` ignores all main-window input
    /// while a dialog is open, so `Z` did nothing either. Not a hang: a modal
    /// nobody could see, with the keyboard locked out. Force quit was the only
    /// way back, which is exactly what was reported.
    /// Hold a file-panel request back for exactly one frame, once.
    ///
    /// Returns whether the caller should give up this frame and try again on
    /// the next. See the call site for why a frame has to pass at all.
    fn arm_panel(&mut self, ctx: &egui::Context, request: &ivory_ui::ports::FileRequest) -> bool {
        if self.panel_armed {
            self.panel_armed = false;
            return false;
        }
        if !self.app.native_panel_up() {
            return false;
        }
        self.panel_armed = true;
        self.deferred_file = Some(request.clone());
        ctx.request_repaint();
        true
    }

    fn restore_fullscreen(&mut self, ctx: &egui::Context) {
        if !self.refullscreen {
            return;
        }
        // Still something on screen, or still something waiting to be shown.
        if self.app.open_dialog().is_some()
            || self.deferred_file.is_some()
            || self.deferred_dir.is_some()
        {
            return;
        }
        self.refullscreen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }

    /// Whether a NATIVE file dialog will actually appear.
    ///
    /// **`rfd` cannot tell you this and its failure looks like a cancel.** On
    /// Linux it has two backends compiled in — xdg-desktop-portal, then a
    /// zenity subprocess — and on a box with neither, `pick_file` returns
    /// `None`, which is the same thing it returns when somebody presses
    /// Cancel. The app cannot distinguish them, so a missing portal is a
    /// button that silently does nothing. Diagnosed on Void + XFCE; see
    /// `docs/LINUX-4.16-FINDINGS.md`.
    ///
    /// So it is probed instead, from the two facts that decide it:
    ///
    /// * a portal implementation that provides `FileChooser` — every portal
    ///   backend registers itself in a `.portal` file listing the interfaces
    ///   it implements, and the box in the report had one for Secret and none
    ///   for files, which is why "a portal is installed" is not the question;
    /// * or `zenity` somewhere on `PATH`.
    ///
    /// No new dependency, and wrong only in the direction that costs nothing:
    /// if this says no and a native dialog would have worked, the in-app
    /// browser opens instead and still chooses the file.
    #[cfg(target_os = "linux")]
    fn native_dialogs_work() -> bool {
        fn which_on_path(exe: &str) -> bool {
            std::env::var_os("PATH").is_some_and(|p| {
                std::env::split_paths(&p).any(|d| d.join(exe).is_file())
            })
        }

        // A session bus at all. Without one there is no portal by definition.
        let has_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
        let portal_dirs = [
            std::path::PathBuf::from("/usr/share/xdg-desktop-portal/portals"),
            std::path::PathBuf::from("/usr/local/share/xdg-desktop-portal/portals"),
        ];
        let chooser = has_bus
            && portal_dirs.iter().any(|dir| {
                std::fs::read_dir(dir).is_ok_and(|rd| {
                    rd.flatten().any(|e| {
                        e.path().extension().is_some_and(|x| x == "portal")
                            && std::fs::read_to_string(e.path())
                                .is_ok_and(|t| t.contains("FileChooser"))
                    })
                })
            });
        chooser || which_on_path("zenity")
    }

    #[cfg(not(target_os = "linux"))]
    fn native_dialogs_work() -> bool {
        true
    }

    /// The directory the in-app browser should show, and its rows.
    ///
    /// Directories first and then files, each sorted case-insensitively,
    /// because that is the order every file manager uses and the one a hand
    /// reaching down a list expects.
    #[cfg(feature = "recorder")]
    fn list_dir(
        at: &std::path::Path,
        extensions: &[String],
    ) -> (Vec<ivory_ui::dialogs::FileEntry>, String) {
        use ivory_ui::dialogs::FileEntry;
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let read = match std::fs::read_dir(at) {
            Ok(r) => r,
            Err(e) => return (Vec::new(), format!("cannot open that folder ({e})")),
        };
        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // Dotfiles stay hidden. Somebody who needs one can still reach it,
            // because the browser follows wherever it is pointed.
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                dirs.push(FileEntry { name, path, is_dir });
            } else if extensions.is_empty()
                || path.extension().is_some_and(|x| {
                    let x = x.to_string_lossy().to_lowercase();
                    extensions.iter().any(|e| e.eq_ignore_ascii_case(&x))
                })
            {
                files.push(FileEntry { name, path, is_dir });
            }
        }
        let key = |e: &FileEntry| e.name.to_lowercase();
        dirs.sort_by_key(key);
        files.sort_by_key(key);
        // The way up first, when there is one.
        let mut out = Vec::with_capacity(dirs.len() + files.len() + 1);
        if let Some(up) = at.parent() {
            out.push(FileEntry {
                name: "..".to_owned(),
                path: up.to_path_buf(),
                is_dir: true,
            });
        }
        out.extend(dirs);
        out.extend(files);
        (out, String::new())
    }

    /// Decode an audio file and hand it to the engine.
    ///
    /// **Decoded once, here, and held as one `Arc`.** The renderer must never
    /// allocate, and a hundred megabytes of `Vec<f32>` is the largest thing
    /// this app ever holds — so it is built on this thread, handed over behind
    /// a pointer, and the old one is dropped here when the new one lands.
    #[cfg(feature = "recorder")]
    fn load_track(&mut self, file: &std::path::Path) {
        let rate = self
            .recorder
            .engine
            .as_ref()
            .map_or(48_000, |e| e.output().sample_rate);
        match ivory_record::decode::decode(file, rate) {
            Ok(clip) => {
                let clip = Arc::new(clip);
                self.app.set_track_info(ivory_ui::ports::TrackInfo {
                    name: clip.label(),
                    seconds: clip.seconds(),
                    wave: clip.envelope(TRACK_WAVE_BUCKETS),
                    error: String::new(),
                });
                self.app
                    .set_track_path(file.to_string_lossy().into_owned());
                if let Some(e) = self.recorder.engine.as_ref() {
                    e.set_track(Some(Arc::clone(&clip)));
                }
                self.track = Some(clip);
            }
            // The name is kept out of the info on purpose: a row that says a
            // file name next to an error reads as "this is loaded but broken",
            // and nothing is loaded.
            Err(error) => {
                self.app.set_track_info(ivory_ui::ports::TrackInfo {
                    error,
                    ..Default::default()
                });
            }
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
    /// Bring the whole audio path up: the OUTPUT first, with no input
    /// callback running, and the input after it.
    ///
    /// **The order is the whole of this function, and it is a deadlock fix.**
    ///
    /// Changing the buffer size from 64 to 128 hung the app on macOS with a
    /// spinning cursor and no way out but a force quit. The sample is
    /// unambiguous:
    ///
    /// ```text
    /// main thread  AudioOutputUnitStart -> StartIOProc
    ///              -> HALB_Mutex::Lock                    [blocked]
    /// IO thread    our input proc -> AudioUnitGetProperty
    ///              -> recursive_mutex::lock               [blocked]
    /// ```
    ///
    /// The input stream was still RUNNING while the output was rebuilt. Both
    /// are the same interface on the ordinary rig, so starting the output sets
    /// that device's buffer frame size — and the input's next callback then
    /// arrives with a frame count that does not match the one `coreaudio-rs`
    /// cached when the stream was built. Its input proc handles that by asking
    /// CoreAudio for the stream format FROM THE AUDIO CALLBACK, and
    /// reallocating there, which is its own sin. So the IO thread holds the
    /// HAL's lock and blocks on the unit's, while the main thread holds the
    /// unit's and blocks on the HAL's. Neither half of that is ours to fix.
    ///
    /// Closing the input first removes the window rather than racing it: no IO
    /// proc is running while the device is reconfigured. Rebuilding the input
    /// LAST matters just as much — the frame size it caches is then the size
    /// the device already has, so the branch cannot fire on the way back up
    /// either.
    ///
    /// The output has no equivalent hazard: its render callback is handed a
    /// buffer and never asks CoreAudio for anything.
    ///
    /// Every path that starts the engine goes through here, because a rule
    /// about ordering that only two of three callers follow is not a rule.
    fn open_audio_path(&mut self, ctx: &egui::Context) {
        self.recorder.session.close_input();
        self.start_engine(ctx);
        self.reconcile_audio(true);
    }

    fn start_engine(&mut self, ctx: &egui::Context) {
        if self.recorder.engine.is_some() {
            return;
        }
        match crate::instrument::Engine::start_sized(
            None,
            self.recorder.session.timebase(),
            self.app.buffer_frames(),
            self.app.sample_rate(),
        ) {
            Ok(e) => {
                self.recorder.engine = Some(e);
                self.recorder.engine_error = None;
                self.recorder.engine_retry = None;
                self.recorder.plugin_loaded = [const { None }; ivory_ui::recorder::SLOTS];
                // **The tap belongs to the ENGINE, so it is taken when the
                // engine starts.**
                //
                // It used to be taken in one place only: the success branch of
                // loading a VST3. Everything else that reaches the instrument
                // bus — the built-in DX7, the backing track, the click going
                // into the take — therefore had no path into the file at all,
                // and the failure was silent because the monitor still played
                // it. Somebody could load the built-in, play, hear it, watch
                // the meters move, and get a take with only the microphone in
                // it; the `.mid` even had every note. Twelve of twelve
                // manifests on the owner's Linux box said `sources: input`,
                // across three releases, for exactly this reason.
                //
                // `take_recorder_tap` is once-only (`Option::take`), so the
                // VST3 branch's own call is now a no-op that costs nothing and
                // is left where it is: it is the line that says the take has to
                // be able to record what it can hear.
                if let Some(tap) =
                    self.recorder.engine.as_mut().and_then(|e| e.take_recorder_tap())
                {
                    self.recorder.session.set_plugin_tap(Some(tap));
                }
                self.push_monitor_settings();
                // **Video defaults for a machine with no GPU driver.**
                //
                // Asked here rather than at construction because the probe
                // enumerates drivers, and a launch that never opens the band
                // never films anything. Once ever, and only from the shipped
                // defaults — see `lower_video_defaults_for_cpu`.
                if crate::composite::renders_on_the_cpu()
                    && self.app.lower_video_defaults_for_cpu()
                {
                    log::info!(
                        "no GPU driver for video: composite defaults lowered to 720p/15"
                    );
                }
                // **The chosen patch, now that there is something to play it.**
                // The cartridge is read at construction, long before the band
                // opens a device, so a voice pushed then goes nowhere — and a
                // fresh install would play the patch written in the source
                // rather than the bank it ships with.
                if let Some(v) = self.chosen_voice() {
                    if let Some(e) = self.recorder.engine.as_mut() {
                        e.set_builtin_voice(v);
                    }
                }
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
        // **The live tap, if one has been waiting for an engine to appear.**
        //
        // First, and before the `&Engine` borrow below, because handing the
        // ring over needs `&mut`. Here rather than on an edge because this runs
        // every frame the band is open: whenever both halves exist, they are
        // joined, and neither one has to know which was built first.
        if let Some(tap) = self.recorder.pending_monitor.take() {
            match self.recorder.engine.as_mut() {
                Some(e) => e.set_monitor(Some(tap)),
                // No engine yet — put it back and try next frame.
                None => self.recorder.pending_monitor = Some(tap),
            }
        }
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
        e.set_master_gain(gains.master);
        // **The channel that had no fader before there was a mixer**, and the
        // routing that decides what reaches the effects bus. Pushed whole
        // every frame, like the gains above and for the same reason.
        e.set_fx_return(gains.fx_return);
        e.set_desk(&self.app.desk());
        // **The microphone fader, and it goes to ONE place now.** It used to
        // go to two — the session's writer thread and the engine — because the
        // writer was where the dry capture became the take. The take is the
        // desk, so the desk's own fader is the whole of it, and the second
        // copy would have been the fader applied to everything in the file
        // rather than to the microphone.
        for (i, g) in gains.inputs.iter().enumerate() {
            e.set_monitor_gain(i, *g);
        }
        // Never read from a settings file — see `IvoryApp::input_monitor`. It
        // is pushed every frame like every other monitor setting, and its value
        // at launch is false because the field it comes from starts false.
        e.set_monitor_on(self.app.input_monitor());
        // The backing track's level and trim, pushed with the rest for the
        // same reason: the settings are the one live value, so a fader moved,
        // a project loaded and a hand-edited file all arrive by one path.
        let (track_gain, from_s, to_s) = self.app.track_playback();
        e.set_track_gain(track_gain);
        let rate = f64::from(e.output().sample_rate);
        // Seconds to frames HERE, because seconds are what survives a machine
        // whose device runs at a different rate than the one that set them.
        let frames = |s: f64| (s.max(0.0) * rate) as u64;
        e.set_track_trim(frames(from_s), frames(to_s));
        // **Rolling with the transport — and only while it is Rolling.**
        //
        // Not during the count-in: the count-in is what counts you IN to the
        // track, and a track that started under it would have its downbeat a
        // bar away from the one being clicked. Not during `Finishing` either,
        // which is a file flushing after the performance ended.
        //
        // Starting at the same instant the take starts writing is also what
        // makes the two line up: the backing track and the recorded audio
        // begin on the same sample.
        e.set_track_playing(matches!(
            self.recorder.session.state(),
            ivory_ui::recorder::RecordState::Rolling
        ));
        // Pushed every frame with the gains, and for the same reason: the
        // settings are the one live value, so a knob dragged, a project loaded
        // and a settings file hand-edited all arrive by the same path.
        let [reverb, delay, chorus, hpf, lpf, limiter] = self.app.effect_sends();
        e.set_effects(crate::effects::Sends {
            reverb,
            delay,
            chorus,
            hpf,
            lpf,
            limiter,
        });
        // The parameters too, and `set_effect_params` is what makes this cheap:
        // it compares before it takes the lock, so pushing the same eleven
        // numbers sixty times a second costs one comparison.
        e.set_effect_params(effect_params_from(self.app.effect_params()));
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

    /// What this take is going to be missing, and where to fix it.
    ///
    /// **Only sources that EXIST.** A muted slot with no instrument in it is
    /// not a loss, it is an empty channel — and naming five of those every
    /// take is how a status line stops being read.
    ///
    /// Named individually rather than counted, because "a source is muted" on
    /// a desk with eight of them is not an instruction anybody can follow.
    fn muted_out_of_the_take(&self) -> Option<String> {
        if !self.recorder.session.is_recording() {
            return None;
        }
        use ivory_ui::recorder::Strip;
        let desk = self.app.desk();
        let engine = self.recorder.engine.as_ref();
        let mut lost: Vec<Strip> = Vec::new();
        for i in 0..ivory_ui::recorder::SLOTS {
            // Loaded, not merely chosen: a slot whose plugin failed to open is
            // already being reported by the row that failed.
            let loaded = self.app.chosen_plugin(i) == Some(ivory_ui::dialogs::BUILTIN_PATH)
                || engine.is_some_and(|e| e.plugin(i).is_some());
            if loaded && !desk.heard(Strip::Slot(i)) {
                lost.push(Strip::Slot(i));
            }
        }
        // Every input that is OPEN, by name. A strip nobody has filled is not
        // a loss; a microphone that is plugged in and muted is.
        for i in 0..ivory_ui::recorder::INPUTS {
            let open = self.recorder.input_names.get(i).is_some_and(|n| !n.is_empty());
            if open && !desk.heard(Strip::Input(i)) {
                lost.push(Strip::Input(i));
            }
        }
        if engine.is_some_and(crate::instrument::Engine::track_loaded)
            && !desk.heard(Strip::Track)
        {
            lost.push(Strip::Track);
        }
        ivory_ui::recorder::missing_from_take(&lost)
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
        // **The built-in is not a bundle.** Its path is a sentinel that has
        // travelled through the picker, the settings and the saved session as
        // if it were one, which is what kept every one of those layers from
        // needing to know it exists. This is the one place that looks.
        if wanted.as_deref() == Some(ivory_ui::dialogs::BUILTIN_PATH) {
            e.unload_plugin(slot);
            e.set_builtin_slot(Some(slot));
            self.recorder.engine_error = None;
            self.recorder.plugin_loaded[slot] = wanted;
            return;
        }
        if self.recorder.plugin_loaded[slot].as_deref()
            == Some(ivory_ui::dialogs::BUILTIN_PATH)
        {
            e.set_builtin_slot(None);
        }
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
                    //
                    // **Named, and with somewhere to go.** "could not load the
                    // instrument" over a rack of five rows does not say WHICH,
                    // and a tester who has just chosen something reads a bare
                    // failure as the app being broken rather than that file
                    // being unsuitable.
                    let which = std::path::Path::new(path)
                        .file_name()
                        .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                    self.recorder.engine_error = Some(format!(
                        "{which} did not load: {err} - Tangent DX7 in the same \
                         list is the built-in instrument"
                    ));
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
        // used to be here was a bug: choosing "None - record without video"
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
        // **The extras, pushed before the staleness check reads it.** Choosing
        // a second input changes how many channels the stream carries and
        // which ones, so it is a reopen — `set_extra_inputs` marks the
        // selection unsettled and this call is what notices.
        crate::devices::set_extra_inputs(&self.recorder.audio, self.app.extra_inputs().to_vec());
        let stale = {
            let sel = crate::devices::selection(&self.recorder.audio);
            sel.is_stale()
        };
        if !stale && !force {
            return;
        }
        match crate::devices::audio_selection(&self.recorder.audio) {
            Some(choice) => {
                self.recorder.session.open_input(
                    &choice.selection,
                    choice.channels,
                    self.app.buffer_frames(),
                    self.app.sample_rate(),
                );
                // **The live tap, HELD until there is an engine to play it.**
                //
                // Taking it and dropping it on the floor is what the first
                // draft did, and the tap is made once per device open — so
                // whenever the engine did not exist at this exact moment,
                // monitoring was silently dead for the rest of the session
                // with no way to get it back short of re-picking the device.
                // That is not a rare window: `start_engine` fails and retries
                // on a busy output, and every retry lands here after the input
                // is already open.
                if let Some(tap) = self.recorder.session.take_monitor() {
                    self.recorder.pending_monitor = Some(tap);
                }
            }
            // The user picked "None - record MIDI only". Mapping that to the
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
                app.exposed_input_channels(),
                app.extra_inputs(),
            );
            // No `explicitly_off` for the camera: absent already means no
            // camera there, because opening one turns on a light and a camera
            // nobody asked for must never be opened.
            crate::devices::restore(&camera, app.chosen_camera_uid(), false, &[], &[]);
            app.set_capture_devices(Some(Box::new(inputs)));
            app.set_cameras(Some(Box::new(cams)));
            app.set_audio_setup(Some(Box::new(crate::devices::Setup::new(&audio))));
            // **The saved system, before anything opens.** Every device lookup
            // in `ivory_record::audio` goes through it, including the one two
            // lines above this that restored the microphone. Left to the
            // per-frame edge in `frame()` it would still be applied, one frame
            // late — and one frame late means the engine starts on the platform
            // default and is then torn down and restarted, which is a five-
            // second instrument reload at every launch.
            ivory_record::audio::set_system(app.audio_system().as_deref());
            // Every installed VST3, by path and file name. This is a DIRECTORY
            // LISTING, not a scan: nothing is opened, so it costs milliseconds
            // even with 112 of them, and no plugin gets the chance to crash the
            // process before the window has appeared. A bundle is opened only
            // when the user picks it.
            let extra = app.plugin_folders();
            app.set_plugin_list(ivory_host::discover_in(&extra));
            Recorder {
                pending_monitor: None,
                input_names: std::array::from_fn(|_| String::new()),
                take_note: None,
                session: crate::record::Session::new(tap, timebase),
                audio,
                camera,
                camera_denied,
                engine: None,
                engine_error: None,
                plugin_loaded: [const { None }; ivory_ui::recorder::SLOTS],
                inserts_loaded: vec![
                    None;
                    (ivory_ui::recorder::STRIPS + 1) * ivory_ui::recorder::INSERTS
                ],
                plugin_opening: None,
                camera_opening: false,
                camera_silent_since: None,
                preview: None,
                preview_px: egui::Vec2::ZERO,
                band_was_open: false,
                disk_checked_at: None,
                disk_bytes: None,
                dev_editor_at: None,
                video: None,
                video_tried: false,
                camera_rgba: None,
                engine_retry: None,
                armed_downbeat: None,
                // Seeded from the settings, not left at None: these are the
                // values the streams are about to be opened with, and a
                // tracker that starts out disagreeing with reality reports a
                // change on frame one that nobody made.
                buffer_open: app.buffer_frames(),
                rate_open: app.sample_rate(),
                system_open: app.audio_system(),
                seen_take: None,
                dev_editor_done: false,
            }
        };

        let mut me = Self {
            app,
            #[cfg(feature = "recorder")]
            recorder,
            splash: Some(Splash {
                since: std::time::Instant::now(),
                done_at: None,
            }),
            #[cfg(feature = "recorder")]
            cartridge: None,
            #[cfg(feature = "recorder")]
            editing: None,
            #[cfg(feature = "recorder")]
            refullscreen: false,
            #[cfg(feature = "recorder")]
            panel_armed: false,
            deferred_file: None,
            #[cfg(feature = "recorder")]
            deferred_dir: None,
            #[cfg(feature = "recorder")]
            reported_take: None,
            #[cfg(feature = "recorder")]
            browse_extensions: Vec::new(),
            #[cfg(feature = "recorder")]
            track: None,
        };
        // What the effects ship as, so the panel can draw a slider for a
        // parameter nobody has moved. See `ports::EffectDefaults`: the DSP owns
        // these numbers and the UI cannot reach them.
        #[cfg(feature = "recorder")]
        me.app.set_effect_defaults(effect_defaults());
        // The cartridge from last time, if it is still where it was. **Silently
        // when it is not**: cartridges live in sample folders that get
        // reorganised, and an error dialog at launch about a file somebody
        // loaded weeks ago is a worse outcome than quietly playing the built-in
        // patch, which is what a fresh install plays anyway.
        #[cfg(feature = "recorder")]
        me.load_cartridge_at_launch();
        me.load_track_at_launch();
        me
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
        // Before the frame, not after: the app decides whether to raise the
        // Welcome card while it paints, and a flag set afterwards would be one
        // frame stale — which is exactly one frame of card over wordmark.
        self.app.set_splash_up(self.splash.is_some());
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
/// The device the WINDOW is drawing with, when there is one to borrow.
///
/// eframe only exposes this when it was built with its `wgpu` feature, which is
/// macOS here: everywhere else the window draws with glow and there is nothing
/// to borrow. `None` is not a refusal any more, it is the compositor opening an
/// adapter of its own, so this is a fast path rather than a gate.
#[cfg(all(feature = "recorder", target_os = "macos"))]
fn window_device(frame: &eframe::Frame) -> Option<&egui_wgpu::RenderState> {
    frame.wgpu_render_state()
}

#[cfg(all(feature = "recorder", not(target_os = "macos")))]
fn window_device(_frame: &eframe::Frame) -> Option<&egui_wgpu::RenderState> {
    None
}

#[cfg(feature = "recorder")]
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
    /// Ticks filled by repeating the previous frame because the machine could
    /// not composite in real time. The video stays on the wall clock; this is
    /// the honest count of how much of it is a freeze-frame.
    padded: u64,
    /// Ticks the take's own clock has passed, whether or not one was produced.
    ///
    /// **Not `next`.** `next` is the timeline position and it can fall behind:
    /// the burst that fills a gap is capped at a second of video, so on a
    /// machine that cannot keep up the deficit carries — and reporting `next`
    /// as "frames expected" quietly shortened the expectation to match what
    /// was delivered. The take then said it had lost nothing while losing
    /// half. This is the honest denominator.
    due: u64,
    /// Compose only every Nth tick, and pad the rest. 1 is every frame.
    ///
    /// Raised when a pump overruns its budget, so a machine that cannot keep
    /// up gives back the UI's time instead of spending it — see the pump. The
    /// video's clock is unaffected; its motion is coarser and `padded` says by
    /// how much.
    stride: u32,
    /// Whether the file carries an audio track, for the manifest's report.
    has_audio: bool,
    /// `Session::camera_frames_delivered` at take start. The camera opens with
    /// the band, not the take, so the per-take figure is a difference.
    cam_frames_at_start: u64,
    /// The same baselines for the three loss counters, so the manifest reports
    /// what happened during THIS take rather than since the camera opened.
    cam_superseded_at_start: u64,
    cam_unreadable_at_start: u64,
    cam_skipped_at_start: u64,
}

#[cfg(feature = "recorder")]
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
        if !spec.produces_video(self.recorder.session.camera_running()) {
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
            match crate::composite::Compositor::new(window_device(frame), video.width, video.height) {
                Ok(c) => c,
                Err(e) => {
                    self.recorder.engine_error = Some(format!("no video this take: {e}"));
                    return;
                }
            };
        // The offscreen context needs the app's fonts or every chord name in
        // the video renders in egui's default face.
        self.app.install_fonts(compositor.context());
        let has_audio = audio.is_some();
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
            padded: 0,
            due: 0,
            stride: 1,
            has_audio,
            cam_frames_at_start: self.recorder.session.camera_frames_delivered(),
            cam_superseded_at_start: self.recorder.session.camera_frames_superseded(),
            cam_unreadable_at_start: self.recorder.session.camera_frames_unreadable(),
            cam_skipped_at_start: self.recorder.session.camera_frames_skipped(),
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
        // The tick whose presentation time has arrived. Everything below is
        // about keeping `v.next` caught up to this number by the END of every
        // pump, because the timestamps come from `v.next` — a pump that leaves
        // ticks unproduced does not make a shorter video, it makes one whose
        // clock runs slow: each late frame carries late content at an early
        // timestamp, and a machine compositing at half speed used to squeeze
        // a whole performance into half its real duration.
        let due = (elapsed * f64::from(v.fps)) as u64;
        v.due = v.due.max(due);

        // Fresh frames first — capped by COUNT for the window-is-slow case (a
        // plugin drag at 8 fps still needs ~4 video frames per pump, and they
        // are cheap when the machine is fast), and by WALL TIME for the
        // machine-is-slow case: three 80 ms composites in one pump is the
        // classic spiral of death, a UI at 4 fps compositing ever further
        // behind. The first composite always runs; the budget only stops a
        // pump from doubling down on a machine that is already saturated.
        const MAX_PER_FRAME: u32 = 3;
        const BUDGET: std::time::Duration = std::time::Duration::from_millis(20);
        // **Input has priority: composite fewer frames rather than steal the
        // UI's time.** A machine that cannot composite at the asked-for rate
        // used to keep trying and pad the difference, which spends the whole
        // budget every pump and leaves the window at four frames a second with
        // notes queued behind it. Above `stride`, only every Nth tick is
        // composed and the rest are padded — the video's clock is unchanged,
        // its motion is coarser, and the window comes back.
        //
        // Doubling and halving rather than a fine control, because it is
        // reacting to a measurement made three times a pump: a fine one would
        // hunt. It stops at 4, which is 15 fps of real frames out of 60 and
        // already far past where a video is worth degrading further.
        //
        // A 15 fps video of a good take beats a 30 fps video of an unplayable
        // one — the owner's own words, and the reason this is not a setting.
        let pump_started = std::time::Instant::now();
        let mut made = 0;
        while v.next < due && made < MAX_PER_FRAME {
            // Behind, and composing only every `stride`th tick: pad this one
            // and move on without paying for a frame nobody asked for.
            if v.stride > 1 && !v.next.is_multiple_of(u64::from(v.stride)) {
                if let Some(last) = v.compositor.last_frame() {
                    let pts = (v.next as i64 * 1_000_000_000) / i64::from(v.fps);
                    if v.encoder.push(last, pts).is_err() {
                        v.failed += 1;
                    }
                    v.padded += 1;
                }
                v.next += 1;
                continue;
            }
            let pts = (v.next as i64 * 1_000_000_000) / i64::from(v.fps);
            let frame = self.recorder.camera_rgba.as_ref().map(|(px, w, h)| (px.as_slice(), *w, *h));
            match v
                .compositor
                .frame(&self.app, v.layout, v.shows, v.camera, v.display, frame, pts)
            {
                // A frame from one tick ago is now readable, carrying its own
                // pts. The readback is pipelined so the UI thread never waits
                // on the rasteriser — see `composite`'s module docs.
                Ok(Some(ready)) => {
                    let pushed = v
                        .compositor
                        .last_frame()
                        .is_some_and(|bgra| v.encoder.push(bgra, ready).is_ok());
                    if !pushed {
                        v.failed += 1;
                    }
                }
                // The first tick of a take: submitted, nothing to encode yet.
                Ok(None) => {}
                Err(_) => v.failed += 1,
            }
            v.next += 1;
            made += 1;
            if pump_started.elapsed() > BUDGET {
                break;
            }
        }
        // What that pump cost, and what to do about it next time.
        let spent = pump_started.elapsed();
        if spent > BUDGET {
            v.stride = (v.stride * 2).min(4);
        } else if spent * 3 < BUDGET && v.stride > 1 {
            // Well under, for a while: try more frames again. A third of the
            // budget rather than half, so a machine sitting exactly on the
            // boundary settles instead of oscillating.
            v.stride /= 2;
        }

        // Still behind: the machine cannot composite this fast, so hold the
        // timeline instead of falling off it — the missed ticks repeat the
        // frame just made. A repeated frame is a visible stutter and an honest
        // one (`v.padded` reports it); a slow clock is invisible and wrong.
        // The burst is capped at a second of video: after a long UI stall the
        // encoder's queue is the wrong place to shove a gigabyte of
        // duplicates, and the deficit carries to the next pump.
        if v.next < due {
            let target = due.min(v.next + u64::from(v.fps));
            if let Some(last) = v.compositor.last_frame() {
                while v.next < target {
                    let pts = (v.next as i64 * 1_000_000_000) / i64::from(v.fps);
                    if v.encoder.push(last, pts).is_err() {
                        v.failed += 1;
                    }
                    v.next += 1;
                    v.padded += 1;
                }
            }
        }
        self.recorder.video = Some(v);
    }

    /// Close the video file. Must happen, or the container has no index.
    fn end_video(&mut self) {
        let Some(mut v) = self.recorder.video.take() else {
            return;
        };
        // **The frame still in the pipeline.** The readback is one deep, so
        // there is always exactly one composited frame that has been submitted
        // and not yet read; without this the last frame of every take is lost.
        // See `composite`'s module docs.
        if let Ok(Some(pts)) = v.compositor.flush() {
            let pushed = v
                .compositor
                .last_frame()
                .is_some_and(|bgra| v.encoder.push(bgra, pts).is_ok());
            if !pushed {
                v.failed += 1;
            }
        }
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
                // **The camera's own shortfall comes first, because it has a
                // different cause and a different fix.**
                //
                // A UVC webcam integrates for as long as the light needs and
                // cannot produce frames faster than that, so in a dim room it
                // silently halves its rate — measured on the owner's machine,
                // `fps = min(30, 1/exposure)` exactly: 66.6 ms of exposure is
                // 15 fps against a negotiated 29.97. Nothing said so, and the
                // frames that never arrived were indistinguishable from frames
                // the take lost. Telling somebody to lower the video size when
                // the room is dark is advice that cannot work.
                //
                // **Only when it actually cost THIS take.** The camera
                // negotiates its own rate and the take composites at its own,
                // and those are different numbers: a camera at 15 feeding a
                // 15 fps take has not cost anybody anything, and saying so
                // after every take would be crying wolf. It becomes true the
                // moment the take asks for more than the room can give.
                let starved = self
                    .recorder
                    .session
                    .camera_rate_limited()
                    .filter(|r| r.actual_fps < f64::from(v.fps) * 0.9);
                if let Some(r) = starved {
                    self.recorder.take_note = Some(format!(
                        "camera gave {:.0} fps of the {} this take wanted - it \
                         needs more light",
                        r.actual_fps, v.fps
                    ));
                } else if dropped > 0 || v.padded > 0 {
                    // Both numbers, because they are different failures: a
                    // dropped frame is a hole, a padded one is a freeze —
                    // and both mean the same advice about this machine.
                    let mut what = Vec::new();
                    if dropped > 0 {
                        what.push(format!("{dropped} video frames lost"));
                    }
                    if v.padded > 0 {
                        what.push(format!("{} repeated", v.padded));
                    }
                    // **Short, and it names the right panel.** The old one
                    // ran to about 155 characters, which `fit_text` shrank
                    // past its own floor — so on a narrower window the line
                    // that says why the take is wrong drew NOTHING at all. And
                    // it sent the user to Take Settings, where the resolution
                    // is not: that is the Export dialog.
                    self.recorder.take_note = Some(format!(
                        "{} - lower the video size in Export",
                        what.join(", ")
                    ));
                }
                // The manifest was written at Stop, before this file existed;
                // fold the finished video into it. §7's report: what the file
                // is, what the rate implies, and what the camera delivered.
                let (width, height) = v.compositor.size();
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.recorder.session.record_video(
                    ivory_record::take::VideoReport {
                        container: ivory_record::encode::CONTAINER.to_owned(),
                        video_codec: ivory_record::encode::VIDEO_CODEC.to_owned(),
                        audio_codec: if v.has_audio {
                            ivory_record::encode::AUDIO_CODEC.to_owned()
                        } else {
                            String::new()
                        },
                        width,
                        height,
                        fps: f64::from(v.fps),
                        // Ticks scheduled over the take. The pump holds these
                        // to the wall clock, so this IS duration x rate.
                        frames_expected: v.due.max(v.next),
                        frames_received: self
                            .recorder
                            .session
                            .camera_frames_delivered()
                            .saturating_sub(v.cam_frames_at_start),
                        // **The three loss counters, which until now nothing
                        // read outside a unit test.** They are the difference
                        // between "the camera did not send it", "the UI could
                        // not keep up with it" and "we chose not to convert
                        // it" — three different faults with three different
                        // fixes, and a take that stutters is unexplainable
                        // without them.
                        frames_superseded: self
                            .recorder
                            .session
                            .camera_frames_superseded()
                            .saturating_sub(v.cam_superseded_at_start),
                        frames_unreadable: self
                            .recorder
                            .session
                            .camera_frames_unreadable()
                            .saturating_sub(v.cam_unreadable_at_start),
                        frames_skipped: self
                            .recorder
                            .session
                            .camera_frames_skipped()
                            .saturating_sub(v.cam_skipped_at_start),
                    },
                    &file_name,
                );
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

        // What is still being waited on. The splash no longer SAYS it — the
        // lattice is the whole picture — but readiness still depends on it.
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
        ivory_ui::splash::draw(&painter, rect, fade);
        // While it is up, the window must keep repainting — nothing else is
        // asking it to, and a splash that freezes mid-fade because no input
        // arrived is worse than none.
        ctx.request_repaint();
    }
}

