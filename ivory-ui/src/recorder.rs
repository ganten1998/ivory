//! What the Recorder band and the Export dialog are made of, as plain data.
//!
//! This module is the seam between the GUI and the machinery. `ivory-record`
//! owns cameras, audio devices, WAV files and take directories; `ivory-ui` must
//! not be able to reach any of that (see `ivory-ui/Cargo.toml` and
//! `scripts/check-firewall.sh`). So the band does not ASK for anything — it is
//! handed a [`RecorderView`] snapshot each frame by whoever is hosting it, and
//! it paints that.
//!
//! The direction of every field here is worth stating once, because it is what
//! keeps the firewall honest:
//!
//!   * **In** — the binary fills a `RecorderView` from the real recorder.
//!   * **Out** — [`recorder_panel::hit_test`] returns a `Hit`, the app turns it
//!     into a request, and the binary performs it after the frame.
//!
//! Nothing in this file opens, reads or writes anything.
//!
//! # Why some obvious things are `&str` and not computed here
//!
//! The folder name a take will get, and how many minutes fit on the disk, are
//! both things this module could *almost* work out. It deliberately does not.
//! Take naming and sanitisation live in `ivory_record::take` — every Windows
//! reserved name, every path-length edge — and a second implementation here
//! would agree with it right up until someone fixed a bug in one of them. So
//! the band is handed the answer as a string and renders it.
//!
//! See `docs/RECORDER-PLAN.md` §5.

use serde_json::{Map, Value};

/// Where a take is in its life.
///
/// Four states rather than a bool because two of them have to be visibly
/// different from a distance: pre-roll is the one where the user is walking
/// back to the bench, and finishing is the one where the files are still being
/// closed and pulling the power cable would cost the take.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RecordState {
    /// Not recording. The preview is large, the meter is live, and the
    /// destination controls are all reachable.
    #[default]
    Idle,
    /// Counting down before the take starts. Carries seconds remaining as a
    /// float so the ring can animate rather than tick.
    PreRoll { remaining_s: f32 },
    /// Rolling. The take started at the instant this was entered.
    Rolling,
    /// Stop was pressed and the files are being closed.
    ///
    /// Its own state because "the button did nothing" is what a user concludes
    /// from a UI that returns to Idle while a 2 GB file is still flushing.
    Finishing,
}

impl RecordState {
    /// Whether the take is live — pre-roll included, because the clock has
    /// already started and the layout has already switched.
    pub fn is_active(self) -> bool {
        !matches!(self, RecordState::Idle)
    }

    /// Whether audio is actually being written. Pre-roll is not: the whole
    /// point of it is that nothing is captured while the user walks back.
    pub fn is_writing(self) -> bool {
        matches!(self, RecordState::Rolling | RecordState::Finishing)
    }
}

/// One channel's worth of level, as the meter needs it.
///
/// Peak AND rms, because they answer different questions: rms is what the
/// signal sounds like and peak is what will clip. A meter showing only one of
/// them is the reason people record either silence or distortion.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Level {
    /// Instantaneous peak, linear 0.0..=1.0+ (values above 1.0 are real and
    /// must not be clamped away before `clipped` is decided).
    pub peak: f32,
    /// Short-window rms, linear.
    pub rms: f32,
    /// Slow-falling peak hold, linear. Drawn as a line, not a fill.
    pub hold: f32,
}

/// The whole meter, plus the one bit that survives the take.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Meters {
    pub left: Level,
    pub right: Level,
    /// Mono sources paint one bar rather than two identical ones.
    pub mono: bool,
    /// **Latched**, not instantaneous. Once a sample has clipped, this stays
    /// true until the next take is armed, so the report after Stop can say so.
    /// An indicator that clears itself is one the performer never sees, because
    /// they were looking at their hands when it happened.
    pub clipped: bool,
}

impl Meters {
    /// Silent, and never clipped — what the band shows with no input open.
    pub const SILENT: Meters = Meters {
        left: Level {
            peak: 0.0,
            rms: 0.0,
            hold: 0.0,
        },
        right: Level {
            peak: 0.0,
            rms: 0.0,
            hold: 0.0,
        },
        mono: false,
        clipped: false,
    };

    /// The loudest peak across whatever channels are live.
    pub fn peak(&self) -> f32 {
        if self.mono {
            self.left.peak
        } else {
            self.left.peak.max(self.right.peak)
        }
    }
}

/// A camera frame that has already been uploaded to the GPU by the host.
///
/// `ivory-ui` never touches a camera: the binary owns the device, converts the
/// frame, and calls `Context::load_texture`. All the band gets is a handle and
/// the frame's real pixel size, which is the only thing it needs to letterbox
/// correctly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preview {
    pub texture: egui::TextureId,
    /// The frame's own size in pixels. **Never reaches the layout** — see
    /// `recorder_panel::fit_preview`. A 4:3 camera must not make the window
    /// taller.
    pub size: egui::Vec2,
}

/// A device slot, as the band renders it.
///
/// Three states and they read differently: no device chosen, a device chosen
/// and open, or a device that was chosen and is now gone (unplugged mid-session
/// is the common one). The third is why this is not `Option<&str>`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeviceLabel<'a> {
    /// Nothing selected. The band offers the picker.
    None,
    /// Open and working.
    Open(&'a str),
    /// Named in settings but not present right now.
    Missing(&'a str),
}

impl<'a> DeviceLabel<'a> {
    pub fn text(self) -> &'a str {
        match self {
            DeviceLabel::None => "None",
            DeviceLabel::Open(n) | DeviceLabel::Missing(n) => n,
        }
    }

    pub fn is_missing(self) -> bool {
        matches!(self, DeviceLabel::Missing(_))
    }
}

/// Everything the band paints, for one frame.
///
/// A borrowed snapshot rather than owned state: it is rebuilt every frame from
/// whatever is true, so there is no second copy of the recorder's state to get
/// out of step with the first.
pub struct RecorderView<'a> {
    pub state: RecordState,
    /// Seconds since the take started writing. Zero in `Idle` and during
    /// pre-roll.
    pub elapsed_s: f64,
    pub meters: Meters,
    /// The output directory, already shortened for display (`~/Movies/Tangent`).
    pub dest: &'a str,
    /// The typed take name. May be empty — the timestamp guarantees uniqueness.
    pub take_name: &'a str,
    /// The name field has keyboard focus, so it draws a caret and the app's
    /// single-letter shortcuts are suppressed.
    pub name_focused: bool,
    /// What the folder will be called, computed by `ivory_record::take`.
    pub folder_preview: &'a str,
    pub camera: DeviceLabel<'a>,
    pub audio: DeviceLabel<'a>,
    pub preview: Option<Preview>,
    /// Recording time left on the destination volume, in minutes, at the
    /// current settings. `None` while it is still being measured.
    ///
    /// A duration and not bytes, deliberately: "214 GB free" means nothing to a
    /// pianist and "~58 min" means everything.
    pub disk_minutes: Option<f64>,
    /// 0, 3 or 5.
    pub preroll_s: u8,
    /// The user asked not to see a clock while playing. §5: a running timer is
    /// the second most-cited performance distraction after a blinking light.
    pub hide_elapsed: bool,
    /// One line of status, shown under the transport. Where "recorded 4:12 to
    /// nocturne-2026-08-16-141203" and "no audio input selected" both go.
    pub message: Option<&'a str>,
    /// Set while the last take clipped, so the report after Stop can say so
    /// even after the meter has been reset.
    pub clip_warning: bool,
}

impl RecorderView<'_> {
    /// A view with nothing configured, for tests and for the first frame.
    pub fn empty() -> RecorderView<'static> {
        RecorderView {
            state: RecordState::Idle,
            elapsed_s: 0.0,
            meters: Meters::SILENT,
            dest: "",
            take_name: "",
            name_focused: false,
            folder_preview: "",
            camera: DeviceLabel::None,
            audio: DeviceLabel::None,
            preview: None,
            disk_minutes: None,
            preroll_s: 3,
            hide_elapsed: false,
            message: None,
            clip_warning: false,
        }
    }
}

/// The owned half of [`RecorderView`], held by the app and filled by the host.
///
/// Two types rather than one because they have opposite lifetimes and opposite
/// owners. This is a long-lived struct the desktop binary writes into once a
/// frame from the real recorder; `RecorderView` is the borrowed snapshot the
/// painter reads. Keeping them separate is what stops the panel from acquiring
/// a `String` field it would then be tempted to mutate mid-draw.
///
/// Every field is inert in a plugin: `Caps::capture_devices` is false there, the
/// band takes zero height, and nothing ever writes to this.
#[derive(Debug, Clone, Default)]
pub struct RecorderState {
    pub state: RecordState,
    pub elapsed_s: f64,
    pub meters: Meters,
    /// The output directory, shortened for display.
    pub dest: String,
    /// What the next take's folder will be called, from `ivory_record::take`.
    pub folder_preview: String,
    pub camera_name: Option<String>,
    /// The camera named in settings is not present right now.
    pub camera_missing: bool,
    pub audio_name: Option<String>,
    pub audio_missing: bool,
    pub preview: Option<Preview>,
    pub disk_minutes: Option<f64>,
    pub message: Option<String>,
    pub clip_warning: bool,
}

impl RecorderState {
    /// Borrow it as the painter wants it.
    ///
    /// The take name, the pre-roll and the hide-elapsed flag come from the app
    /// rather than from here: they are settings and edit state the app already
    /// owns, and a second copy would be a second thing to keep in step.
    pub fn view<'a>(
        &'a self,
        take_name: &'a str,
        name_focused: bool,
        preroll_s: u8,
        hide_elapsed: bool,
    ) -> RecorderView<'a> {
        let label = |name: &'a Option<String>, missing: bool| match name.as_deref() {
            None => DeviceLabel::None,
            Some(n) if missing => DeviceLabel::Missing(n),
            Some(n) => DeviceLabel::Open(n),
        };
        RecorderView {
            state: self.state,
            elapsed_s: self.elapsed_s,
            meters: self.meters,
            dest: &self.dest,
            take_name,
            name_focused,
            folder_preview: &self.folder_preview,
            camera: label(&self.camera_name, self.camera_missing),
            audio: label(&self.audio_name, self.audio_missing),
            preview: self.preview,
            disk_minutes: self.disk_minutes,
            preroll_s,
            hide_elapsed,
            message: self.message.as_deref(),
            clip_warning: self.clip_warning,
        }
    }
}

/// Something the band asked the host to do, drained after the frame.
///
/// The **request pattern**, for the same reason the directory picker uses it:
/// creating a take directory and opening a device must not happen halfway
/// through painting a frame. The plugin refuses simply by never draining.
///
/// Deliberately only two variants. Everything else a click in the band can do —
/// choosing a folder, choosing a device, changing the pre-roll, opening the
/// Export dialog — is either a settings write or a dialog, and both of those
/// are the app's own business. A request enum that carried them would be a
/// second, weaker copy of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderRequest {
    /// The one button. Start a pre-roll, start a take, or stop one — the
    /// session decides which, because only it knows what state it is in.
    Toggle,
    /// Stop, unconditionally. Separate from `Toggle` because the band draws a
    /// distinct Stop control and "the button that stops it" must not also be
    /// able to start one.
    Stop,
}

// ── The export contract ─────────────────────────────────────────────────────

/// How many video files a take produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoMode {
    /// Audio and MIDI only. The default, and not a degraded one: it is what
    /// somebody recording a practice session actually wants.
    #[default]
    None,
    /// One file containing whichever of camera / display / audio are ticked.
    Composite,
    /// One file per source, each on its own.
    PerSource,
    /// Both of the above.
    Both,
}

impl VideoMode {
    pub const ALL: [VideoMode; 4] = [
        VideoMode::None,
        VideoMode::Composite,
        VideoMode::PerSource,
        VideoMode::Both,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VideoMode::None => "None",
            VideoMode::Composite => "One video, composited",
            VideoMode::PerSource => "Separate file per source",
            VideoMode::Both => "Both",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            VideoMode::None => "none",
            VideoMode::Composite => "composite",
            VideoMode::PerSource => "per_source",
            VideoMode::Both => "both",
        }
    }

    fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == k)
    }

    /// Whether a composite file is produced, which is what makes the layout and
    /// "composite contains" controls meaningful.
    pub fn has_composite(self) -> bool {
        matches!(self, VideoMode::Composite | VideoMode::Both)
    }

    pub fn wants_video(self) -> bool {
        !matches!(self, VideoMode::None)
    }
}

/// How the camera and the display share one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Camera on top, Tangent's display below. The default because it is the
    /// shape of every piano tutorial video ever made: hands above, keys below.
    #[default]
    CameraAbove,
    DisplayAbove,
    /// Camera fills the frame with the display floated over the bottom of it.
    CameraFull,
    SideBySide,
}

impl Layout {
    pub const ALL: [Layout; 4] = [
        Layout::CameraAbove,
        Layout::DisplayAbove,
        Layout::CameraFull,
        Layout::SideBySide,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Layout::CameraAbove => "Camera above, display below",
            Layout::DisplayAbove => "Display above, camera below",
            Layout::CameraFull => "Camera full frame, display overlaid",
            Layout::SideBySide => "Side by side",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Layout::CameraAbove => "camera_above",
            Layout::DisplayAbove => "display_above",
            Layout::CameraFull => "camera_full",
            Layout::SideBySide => "side_by_side",
        }
    }

    fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.key() == k)
    }
}

/// The composited video's frame size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    #[default]
    Hd1080,
    Hd720,
    /// Whatever the camera is giving us. Sharpest, and the one that makes the
    /// file size unpredictable, which is why it is not the default.
    MatchCamera,
}

impl Resolution {
    pub const ALL: [Resolution; 3] = [
        Resolution::Hd1080,
        Resolution::Hd720,
        Resolution::MatchCamera,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Resolution::Hd1080 => "1920x1080",
            Resolution::Hd720 => "1280x720",
            Resolution::MatchCamera => "Match camera",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Resolution::Hd1080 => "1080",
            Resolution::Hd720 => "720",
            Resolution::MatchCamera => "camera",
        }
    }

    fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.key() == k)
    }

    /// Pixels, or `None` for "whatever the camera says".
    pub fn pixels(self) -> Option<(u32, u32)> {
        match self {
            Resolution::Hd1080 => Some((1920, 1080)),
            Resolution::Hd720 => Some((1280, 720)),
            Resolution::MatchCamera => None,
        }
    }
}

/// Which of Tangent's own panels appear in the video.
///
/// A separate set from the live `Settings` flags on purpose: recording a clean
/// piano-and-chord video while keeping the fretboard on screen for your own use
/// is a thing people want, and it is one struct field rather than a code path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayShows {
    pub piano: bool,
    pub chord: bool,
    pub fretboard: bool,
    pub theory: bool,
}

impl Default for DisplayShows {
    /// Piano and chord name. The two panels that are about what was just
    /// played, which is what a video of a performance is for.
    fn default() -> Self {
        Self {
            piano: true,
            chord: true,
            fretboard: false,
            theory: false,
        }
    }
}

impl DisplayShows {
    /// Nothing ticked means there is no display layer to composite, which the
    /// dialog has to notice before it offers a layout for it.
    pub fn any(self) -> bool {
        self.piano || self.chord || self.fretboard || self.theory
    }
}

/// What the composited video contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Composite {
    pub camera: bool,
    pub display: bool,
    /// The performance audio, muxed into the video as well as written to the
    /// `.wav`. Off makes a silent video, which is a real request (people who
    /// score to picture) and a common mistake, so it is stated rather than
    /// implied.
    pub audio: bool,
    pub layout: Layout,
    pub shows: DisplayShows,
}

impl Default for Composite {
    fn default() -> Self {
        Self {
            camera: true,
            display: true,
            audio: true,
            layout: Layout::default(),
            shows: DisplayShows::default(),
        }
    }
}

/// Everything the Export dialog decides.
///
/// One struct so that "use these settings for every take" is a single
/// round-trip through the settings file rather than eleven keys that can go out
/// of step with each other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportSpec {
    /// Write the `.wav`.
    pub audio: bool,
    /// Write the `.mid`.
    pub midi: bool,
    /// The tempo written into the SMF's tempo meta event. It does not change a
    /// single timestamp — the file is real time either way — it changes what a
    /// DAW's bar ruler lines up with when you drop the file in.
    pub tempo_bpm: f64,
    pub video: VideoMode,
    pub composite: Composite,
    pub resolution: Resolution,
    pub fps: u32,
}

impl Default for ExportSpec {
    fn default() -> Self {
        Self {
            audio: true,
            midi: true,
            tempo_bpm: 120.0,
            video: VideoMode::default(),
            composite: Composite::default(),
            resolution: Resolution::default(),
            fps: 30,
        }
    }
}

/// Why a spec cannot be exported as it stands.
///
/// Returned rather than silently corrected: a dialog that quietly re-ticks a
/// box the user just unticked is worse than one that refuses and says why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecProblem {
    /// Neither audio nor MIDI. The owner's brief is that a take produces the
    /// audio, the MIDI and the video; a directory with none of the first two is
    /// not a take.
    NothingToWrite,
    /// A composite was asked for with neither layer in it.
    EmptyComposite,
    /// The display layer is on but every panel inside it is off.
    EmptyDisplay,
}

impl SpecProblem {
    pub fn message(self) -> &'static str {
        match self {
            SpecProblem::NothingToWrite => "A take has to write the audio or the MIDI, or both.",
            SpecProblem::EmptyComposite => {
                "The composited video needs the camera, the display, or both."
            }
            SpecProblem::EmptyDisplay => {
                "The display layer is on but no panel is selected to draw in it."
            }
        }
    }
}

impl ExportSpec {
    /// The first thing wrong with this spec, if anything is.
    pub fn problem(&self) -> Option<SpecProblem> {
        if !self.audio && !self.midi {
            return Some(SpecProblem::NothingToWrite);
        }
        if self.video.has_composite() {
            if !self.composite.camera && !self.composite.display {
                return Some(SpecProblem::EmptyComposite);
            }
            if self.composite.display && !self.composite.shows.any() {
                return Some(SpecProblem::EmptyDisplay);
            }
        }
        None
    }

    pub fn is_valid(&self) -> bool {
        self.problem().is_none()
    }

    /// Whether a camera has to be open for this spec to be satisfiable.
    ///
    /// Used to decide whether the camera controls matter, and — after a take —
    /// which options can be offered at all. A take recorded without the camera
    /// can never gain it: those frames were composited live and nothing kept
    /// them. See `docs/RECORDER-PLAN.md` §5.
    pub fn needs_camera(&self) -> bool {
        match self.video {
            VideoMode::None => false,
            VideoMode::PerSource | VideoMode::Both => true,
            VideoMode::Composite => self.composite.camera,
        }
    }

    /// Roughly how many megabytes a minute this spec writes.
    ///
    /// Deliberately crude and deliberately honest about it: it exists to turn
    /// free disk space into a duration, and being 20% out on that changes
    /// nothing anyone does. Audio is exact (48 kHz, 24-bit, stereo = 8.24
    /// MB/min); the video terms are the bitrates the encoders are configured
    /// for, not measurements of a particular scene.
    pub fn megabytes_per_minute(&self) -> f64 {
        let mut mb = 0.0;
        if self.audio {
            // 48000 * 3 bytes * 2 channels * 60 s
            mb += 48_000.0 * 3.0 * 2.0 * 60.0 / 1_000_000.0;
        }
        if self.midi {
            // A busy performance is a few kB a minute. Rounds to nothing, and
            // saying so is better than pretending MIDI has no cost at all.
            mb += 0.01;
        }
        let per_stream = match self.resolution {
            // 12 Mbit/s and 6 Mbit/s, converted to MB per minute.
            Resolution::Hd1080 | Resolution::MatchCamera => 12.0 * 60.0 / 8.0,
            Resolution::Hd720 => 6.0 * 60.0 / 8.0,
        };
        let streams = match self.video {
            VideoMode::None => 0,
            VideoMode::Composite => 1,
            VideoMode::PerSource => {
                usize::from(self.composite.camera) + usize::from(self.composite.display)
            }
            VideoMode::Both => {
                1 + usize::from(self.composite.camera) + usize::from(self.composite.display)
            }
        };
        mb + per_stream * streams as f64
    }

    /// How many simultaneous video encoders this spec runs.
    ///
    /// Shown in the dialog because three 1080p30 encodes is free on
    /// VideoToolbox and Media Foundation and genuinely expensive on Linux's
    /// software encoder, and the user deserves to know which machine they are
    /// on before they find out during a take.
    pub fn encoder_count(&self) -> usize {
        match self.video {
            VideoMode::None => 0,
            VideoMode::Composite => 1,
            VideoMode::PerSource => {
                usize::from(self.composite.camera) + usize::from(self.composite.display)
            }
            VideoMode::Both => {
                1 + usize::from(self.composite.camera) + usize::from(self.composite.display)
            }
        }
    }

    // ── persistence ─────────────────────────────────────────────────────────
    //
    // Stored as ONE nested object under `record_export` rather than the eleven
    // flat `record_export_*` keys the plan sketched. The plan's own reason for
    // flat keys was consistency with the existing file, but this value is
    // written and read as a unit — "use these settings for every take" is one
    // decision — and eleven independent keys can be half-applied by a
    // hand-edited file in ways that produce a spec `problem()` rejects at the
    // moment of recording. A nested object is atomic: present and complete, or
    // absent and default.

    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("audio".into(), Value::Bool(self.audio));
        m.insert("midi".into(), Value::Bool(self.midi));
        m.insert(
            "tempo_bpm".into(),
            serde_json::Number::from_f64(self.tempo_bpm)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
        m.insert("video".into(), Value::String(self.video.key().into()));
        m.insert("camera".into(), Value::Bool(self.composite.camera));
        m.insert("display".into(), Value::Bool(self.composite.display));
        m.insert(
            "composite_audio".into(),
            Value::Bool(self.composite.audio),
        );
        m.insert(
            "layout".into(),
            Value::String(self.composite.layout.key().into()),
        );
        m.insert("show_piano".into(), Value::Bool(self.composite.shows.piano));
        m.insert("show_chord".into(), Value::Bool(self.composite.shows.chord));
        m.insert(
            "show_fretboard".into(),
            Value::Bool(self.composite.shows.fretboard),
        );
        m.insert(
            "show_theory".into(),
            Value::Bool(self.composite.shows.theory),
        );
        m.insert(
            "resolution".into(),
            Value::String(self.resolution.key().into()),
        );
        m.insert("fps".into(), Value::Number(self.fps.into()));
        Value::Object(m)
    }

    /// Per-key forgiving, like every other reader in `settings.rs`: a key that
    /// is missing or the wrong type keeps that field's default rather than
    /// throwing the whole object away.
    pub fn from_value(v: &Value) -> Self {
        let Value::Object(m) = v else {
            return Self::default();
        };
        let mut s = Self::default();
        let b = |k: &str, dst: &mut bool| {
            if let Some(x) = m.get(k).and_then(Value::as_bool) {
                *dst = x;
            }
        };
        b("audio", &mut s.audio);
        b("midi", &mut s.midi);
        b("camera", &mut s.composite.camera);
        b("display", &mut s.composite.display);
        b("composite_audio", &mut s.composite.audio);
        b("show_piano", &mut s.composite.shows.piano);
        b("show_chord", &mut s.composite.shows.chord);
        b("show_fretboard", &mut s.composite.shows.fretboard);
        b("show_theory", &mut s.composite.shows.theory);
        if let Some(t) = m.get("tempo_bpm").and_then(Value::as_f64) {
            if (MIN_BPM..=MAX_BPM).contains(&t) {
                s.tempo_bpm = t;
            }
        }
        if let Some(k) = m.get("video").and_then(Value::as_str) {
            if let Some(v) = VideoMode::from_key(k) {
                s.video = v;
            }
        }
        if let Some(k) = m.get("layout").and_then(Value::as_str) {
            if let Some(l) = Layout::from_key(k) {
                s.composite.layout = l;
            }
        }
        if let Some(k) = m.get("resolution").and_then(Value::as_str) {
            if let Some(r) = Resolution::from_key(k) {
                s.resolution = r;
            }
        }
        if let Some(f) = m.get("fps").and_then(Value::as_u64) {
            if FPS_CHOICES.contains(&(f as u32)) {
                s.fps = f as u32;
            }
        }
        s
    }
}

/// The tempo marks the dialog will accept. A DAW will happily import 0.001 BPM
/// and then draw a bar ruler several centuries long.
pub const MIN_BPM: f64 = 20.0;
pub const MAX_BPM: f64 = 300.0;

/// Frame rates offered. Deliberately short: 30 is what a webcam gives, 24 is
/// what people who want a film look ask for, 60 is what people recording fast
/// passages ask for, and everything else is a support question.
pub const FPS_CHOICES: [u32; 3] = [24, 30, 60];

/// Pre-roll choices, in seconds. §5: enough to walk back to the bench.
pub const PREROLL_CHOICES: [u8; 3] = [0, 3, 5];

/// Format a duration the way a transport does: `M:SS` under an hour,
/// `H:MM:SS` over it.
///
/// Its own function so the band and the post-take message cannot disagree, and
/// because the hour case is exactly the one nobody tests and somebody's
/// two-hour practice session finds.
pub fn timecode(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Turn free bytes into "about this many minutes", the only unit that means
/// anything to someone deciding whether to press record.
///
/// `None` when the spec writes nothing measurable, which would otherwise be a
/// division by zero reported as infinite recording time.
pub fn minutes_on_disk(free_bytes: u64, spec: &ExportSpec) -> Option<f64> {
    let per_min = spec.megabytes_per_minute();
    if per_min <= 0.0 {
        return None;
    }
    Some(free_bytes as f64 / 1_000_000.0 / per_min)
}

/// "~58 min", "~2 h 14 min", "under a minute".
pub fn disk_text(minutes: f64) -> String {
    if minutes < 1.0 {
        "under a minute".to_owned()
    } else if minutes < 90.0 {
        format!("~{} min", minutes.round() as u64)
    } else {
        let h = (minutes / 60.0).floor() as u64;
        let m = (minutes - h as f64 * 60.0).round() as u64;
        format!("~{h} h {m} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_take_that_writes_nothing_is_refused() {
        let mut s = ExportSpec::default();
        assert!(s.is_valid());
        s.audio = false;
        assert!(s.is_valid(), "MIDI alone is a perfectly good take");
        s.midi = false;
        assert_eq!(s.problem(), Some(SpecProblem::NothingToWrite));
        s.audio = true;
        assert!(s.is_valid());
    }

    #[test]
    fn a_composite_of_nothing_is_refused() {
        let mut s = ExportSpec {
            video: VideoMode::Composite,
            ..Default::default()
        };
        assert!(s.is_valid());
        s.composite.camera = false;
        s.composite.display = false;
        assert_eq!(s.problem(), Some(SpecProblem::EmptyComposite));
    }

    /// The subtle one: the display layer is on, so the composite is not empty,
    /// but every panel inside it is off — which composites a black rectangle
    /// over half the frame and looks like a bug in the encoder.
    #[test]
    fn a_display_layer_with_no_panels_is_refused() {
        let s = ExportSpec {
            video: VideoMode::Composite,
            composite: Composite {
                camera: false,
                display: true,
                shows: DisplayShows {
                    piano: false,
                    chord: false,
                    fretboard: false,
                    theory: false,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(s.problem(), Some(SpecProblem::EmptyDisplay));
    }

    /// Only meaningful when a composite is actually being made. A per-source
    /// export with no panels ticked is a camera-only video, which is fine.
    #[test]
    fn the_display_check_does_not_fire_without_a_composite() {
        let s = ExportSpec {
            video: VideoMode::PerSource,
            composite: Composite {
                display: true,
                shows: DisplayShows {
                    piano: false,
                    chord: false,
                    fretboard: false,
                    theory: false,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(s.is_valid());
    }

    #[test]
    fn an_export_spec_survives_a_round_trip() {
        let s = ExportSpec {
            audio: false,
            midi: true,
            tempo_bpm: 92.5,
            video: VideoMode::Both,
            composite: Composite {
                camera: false,
                display: true,
                audio: false,
                layout: Layout::SideBySide,
                shows: DisplayShows {
                    piano: false,
                    chord: true,
                    fretboard: true,
                    theory: true,
                },
            },
            resolution: Resolution::Hd720,
            fps: 60,
        };
        assert_eq!(ExportSpec::from_value(&s.to_value()), s);
    }

    /// The reason the reader validates rather than trusting: a hand-edited
    /// file, or one written by a future version, must not be able to put a
    /// value in that the dialog itself would refuse to produce.
    #[test]
    fn a_hostile_settings_file_cannot_inject_a_nonsense_spec() {
        let v: Value = serde_json::from_str(
            r#"{"tempo_bpm": 0.0001, "fps": 999, "video": "holograph",
                "layout": "diagonal", "resolution": "8k", "audio": "yes"}"#,
        )
        .unwrap();
        let s = ExportSpec::from_value(&v);
        assert_eq!(s, ExportSpec::default(), "every bad key fell back");
    }

    #[test]
    fn nothing_is_wrong_with_an_absent_export_block() {
        assert_eq!(ExportSpec::from_value(&Value::Null), ExportSpec::default());
    }

    /// Encoder count is what the dialog promises the CPU cost from, so it has
    /// to match the file count it also promises.
    #[test]
    fn the_encoder_count_matches_the_number_of_video_files() {
        let both_layers = Composite {
            camera: true,
            display: true,
            ..Default::default()
        };
        let cases = [
            (VideoMode::None, both_layers, 0),
            (VideoMode::Composite, both_layers, 1),
            (VideoMode::PerSource, both_layers, 2),
            (VideoMode::Both, both_layers, 3),
            (
                VideoMode::PerSource,
                Composite {
                    camera: true,
                    display: false,
                    ..Default::default()
                },
                1,
            ),
        ];
        for (mode, comp, want) in cases {
            let s = ExportSpec {
                video: mode,
                composite: comp,
                ..Default::default()
            };
            assert_eq!(s.encoder_count(), want, "{mode:?}");
        }
    }

    /// The camera question decides what a *finished* take can be re-exported
    /// as, so getting it wrong offers the user an option that cannot work.
    #[test]
    fn needing_a_camera_follows_the_video_mode_and_the_tick() {
        let no_cam = Composite {
            camera: false,
            ..Default::default()
        };
        assert!(!ExportSpec::default().needs_camera());
        assert!(ExportSpec {
            video: VideoMode::Composite,
            ..Default::default()
        }
        .needs_camera());
        assert!(!ExportSpec {
            video: VideoMode::Composite,
            composite: no_cam,
            ..Default::default()
        }
        .needs_camera());
    }

    #[test]
    fn audio_only_costs_what_the_arithmetic_says() {
        let s = ExportSpec {
            audio: true,
            midi: false,
            video: VideoMode::None,
            ..Default::default()
        };
        // 48k * 3 bytes * 2ch * 60s = 17.28 MB per minute.
        assert!((s.megabytes_per_minute() - 17.28).abs() < 0.01);
    }

    #[test]
    fn a_take_with_no_content_reports_no_disk_duration() {
        let s = ExportSpec {
            audio: false,
            midi: false,
            video: VideoMode::None,
            ..Default::default()
        };
        assert_eq!(minutes_on_disk(500_000_000_000, &s), None);
    }

    #[test]
    fn the_transport_grows_an_hours_field_when_it_needs_one() {
        assert_eq!(timecode(0.0), "0:00");
        assert_eq!(timecode(9.7), "0:09");
        assert_eq!(timecode(61.0), "1:01");
        assert_eq!(timecode(599.0), "9:59");
        assert_eq!(timecode(3599.0), "59:59");
        assert_eq!(timecode(3600.0), "1:00:00");
        assert_eq!(timecode(7384.0), "2:03:04");
        assert_eq!(timecode(-5.0), "0:00", "a negative clock reads zero");
    }

    #[test]
    fn disk_space_reads_as_a_duration_at_every_scale() {
        assert_eq!(disk_text(0.4), "under a minute");
        assert_eq!(disk_text(58.2), "~58 min");
        assert_eq!(disk_text(134.0), "~2 h 14 min");
    }

    #[test]
    fn a_missing_device_is_not_the_same_as_no_device() {
        assert!(!DeviceLabel::Open("FaceTime HD").is_missing());
        assert!(DeviceLabel::Missing("FaceTime HD").is_missing());
        assert_eq!(DeviceLabel::None.text(), "None");
        assert_eq!(DeviceLabel::Missing("Scarlett").text(), "Scarlett");
    }

    /// Pre-roll is not writing. If it were, every take would open with three
    /// seconds of the user walking across the room.
    #[test]
    fn preroll_is_active_but_not_writing() {
        let p = RecordState::PreRoll { remaining_s: 2.4 };
        assert!(p.is_active() && !p.is_writing());
        assert!(RecordState::Rolling.is_writing());
        assert!(RecordState::Finishing.is_writing());
        assert!(!RecordState::Idle.is_active());
    }
}
