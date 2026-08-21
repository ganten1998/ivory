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
    /// Counting the take in, in BEATS.
    ///
    /// Beats and not seconds because a count-in is a musical instruction: "two
    /// bars" is what a musician asks for and what the click is playing, and a
    /// countdown in seconds against a click in beats is two clocks disagreeing
    /// in front of the person trying to come in on time.
    CountIn { beat: u32, of: u32 },
    /// Rolling. The take started at the instant this was entered.
    Rolling,
    /// Stop was pressed and the files are being closed.
    ///
    /// Its own state because "the button did nothing" is what a user concludes
    /// from a UI that returns to Idle while a 2 GB file is still flushing.
    Finishing,
}

impl RecordState {
    /// Whether the take is live — the count-in included, because the layout has
    /// already switched and Record has already been pressed.
    pub fn is_active(self) -> bool {
        !matches!(self, RecordState::Idle)
    }

    /// Whether audio is actually being written. The count-in is not: the whole
    /// point of it is that the bars before the downbeat are not in the file.
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

/// How many instruments can be layered at once.
///
/// Three, because that is what the owner asked for and because it is the number
/// that covers the real cases: a piano, a pad under it, and something on top.
/// It is a constant rather than a `Vec` so the band can lay out a fixed number
/// of slots — a variable count would make the band's height depend on how many
/// instruments are loaded, and every band's height in this app is a function of
/// width alone.
/// **Five, up from three.** The two monitor faders moved into the transport
/// group, and the column they left is what the extra rows fit in: a layered
/// pad, a bass and a lead is three, and anybody building a rig wanted more than
/// that the first time they tried.
///
/// Not more than five, because the band lost a fifth of its height in the same
/// change. Six rows in the shorter band put the plugin name, the gain reading
/// and the OPEN WINDOW button at four points, which is a row nobody can read —
/// the count is bounded by what a row can legibly hold, not by the space.
pub const SLOTS: usize = 5;

/// One instrument slot, as the band draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotView<'a> {
    /// What is loaded. `None` is an empty slot, which is still drawn — three
    /// visible slots means three, not "as many as are full".
    pub name: Option<&'a str>,
    /// Named in settings but would not load this time. Distinct from empty:
    /// a licence server that was unreachable this morning is not the same as a
    /// slot nobody has filled.
    pub missing: bool,
    /// Linear gain for this slot.
    pub gain: f32,
    /// This instrument offers an editor. A plugin without one is legal VST3.
    pub has_editor: bool,
    /// Its window is on screen.
    pub editor_open: bool,
}

impl SlotView<'_> {
    pub const EMPTY: SlotView<'static> = SlotView {
        name: None,
        missing: false,
        gain: 1.0,
        has_editor: false,
        editor_open: false,
    };

    /// Whether there is an instrument here at all — loaded or merely named.
    pub fn filled(&self) -> bool {
        self.name.is_some()
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
    /// The OUTPUT's levels — after the effects, after the limiter, after the
    /// master, click included. Not the same signal as `meters`, which is what
    /// is being recorded.
    pub master: Meters,
    /// Decibels the limiter is taking off right now, zero for none.
    pub gr_db: f32,
    /// The output directory, already shortened for display (`~/Movies/Tangent`).
    pub dest: &'a str,
    /// The typed take name. May be empty — the timestamp guarantees uniqueness.
    pub take_name: &'a str,
    /// The name field has keyboard focus, so it draws a caret and the app's
    /// single-letter shortcuts are suppressed.
    pub name_focused: bool,
    /// A numeric control being typed into, if any. Draws in place of that
    /// control's reading, with a caret; suppresses the single-key shortcuts for
    /// the same reason the name field does.
    pub editing: Option<&'a NumEdit>,
    /// What the folder will be called, computed by `ivory_record::take`.
    pub folder_preview: &'a str,
    /// The three instrument slots, always all three — an empty one is still
    /// drawn, because "three visible slots" is what makes layering discoverable
    /// rather than a thing you have to know about.
    pub slots: [SlotView<'a>; SLOTS],
    /// The four faders, as LINEAR gains (not fader positions). See
    /// [`gain_to_fader`] for turning one into a knob angle.
    pub gains: Gains,
    /// The six effect knobs, 0..=1, as they are drawn.
    pub fx: FxSends,
    /// What each of those numbers MEANS, in [`crate::recorder_panel::Fx::ALL`]
    /// order. Told by the host; see [`crate::ports::KnobUnit`].
    pub fx_units: [crate::ports::KnobUnit; 6],
    /// The backing track: its name, its length and its outline. Borrowed, not
    /// owned — the outline is a thousand floats and the view is built every
    /// frame.
    pub track: &'a crate::ports::TrackInfo,
    /// The control a hand is on right now, if any. A knob shows its number
    /// while it is being turned and its name the rest of the time: there is
    /// nowhere on a knob to keep both, and the number is only wanted while it
    /// is changing.
    pub turning: Option<NumField>,
    /// The click is on. Independent of `metronome_in_take`.
    pub metronome_on: bool,
    /// The click is mixed into the recording as well as the monitors.
    ///
    /// **Off by default, and that is the important default in this struct.** A
    /// click bleeding into the take is a ruined take, and it is the mistake
    /// nobody notices until they open the file.
    pub metronome_in_take: bool,
    /// The live input is being heard. Never persisted — see
    /// `IvoryApp::input_monitor`.
    pub input_monitor: bool,
    /// Beats and tempo, shared by the click, the count-in and the SMF's tempo
    /// mark — one number, because a click at 90 against a file that says 120
    /// is a take nobody can edit afterwards.
    pub tempo_bpm: f64,
    /// The take's time signature, shown beside the tempo and the count-in
    /// because the three describe one thing between them.
    pub time_signature: TimeSignature,
    /// Count-in length in BARS — what the cell shows and what a click cycles.
    pub count_in_bars: u32,
    pub count_in_beats: u32,
    pub camera: DeviceLabel<'a>,
    pub audio: DeviceLabel<'a>,
    pub preview: Option<Preview>,
    /// Recording time left on the destination volume, in minutes, at the
    /// current settings. `None` while it is still being measured.
    ///
    /// A duration and not bytes, deliberately: "214 GB free" means nothing to a
    /// pianist and "~58 min" means everything.
    pub disk_minutes: Option<f64>,

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
    /// Whether the clip warning is on screen — and therefore whether it is
    /// something a press can land on.
    pub fn showing_clip(&self) -> bool {
        self.meters.clipped || self.clip_warning
    }

    /// A view with nothing configured, for tests and for the first frame.
    pub fn empty() -> RecorderView<'static> {
        RecorderView {
            input_monitor: false,
            state: RecordState::Idle,
            elapsed_s: 0.0,
            meters: Meters::SILENT,
            master: Meters::SILENT,
            gr_db: 0.0,
            dest: "",
            take_name: "",
            name_focused: false,
            editing: None,
            folder_preview: "",
            slots: [SlotView::EMPTY; SLOTS],
            gains: Gains::default(),
            fx: FxSends { reverb: 0.0, delay: 0.0, chorus: 0.0, hpf: 0.0, lpf: 0.0, limiter: 0.0 },
            fx_units: [crate::ports::KnobUnit::Percent; 6],
            track: crate::ports::TrackInfo::NONE,
            turning: None,
            metronome_on: false,
            metronome_in_take: false,
            tempo_bpm: DEFAULT_BPM,
            time_signature: TimeSignature::default(),
            count_in_bars: 1,
            count_in_beats: 4,
            camera: DeviceLabel::None,
            audio: DeviceLabel::None,
            preview: None,
            disk_minutes: None,
            hide_elapsed: false,
            message: None,
            clip_warning: false,
        }
    }
}

/// The four things with a volume control, as linear gains.
///
/// Linear rather than dB because that is what the audio path multiplies by;
/// the band converts for display and for the fader's travel. A struct rather
/// than four loose floats so that adding a fifth source is one field and not
/// five call sites.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gains {
    /// One per instrument slot, so a layered sound can be balanced.
    ///
    /// An array rather than three fields: every consumer wants to iterate them,
    /// and a fourth slot should be a constant change rather than a fourth
    /// field, a fourth fader and a fourth settings key.
    pub slots: [f32; SLOTS],
    /// The click. Applies to what you hear; whether it reaches the FILE is
    /// `metronome_in_take`, which is a separate question with a separate
    /// answer.
    pub metronome: f32,
    /// The inputs being recorded, one fader each.
    ///
    /// **An array for the same reason the slots are one**: the band shows the
    /// first and the mixer shows all of them, and two fields for one number is
    /// how a fader and its second view disagree.
    pub inputs: [f32; INPUTS],
    /// **The master.** Last on the instrument bus, after the limiter, on both
    /// what you hear and what is written. Not the click, which has its own.
    pub master: f32,
    /// The backing track, which rolls with the transport.
    pub track: f32,
    /// What comes back from the effects bus, at its own fader.
    pub fx_return: f32,
}

/// A channel of the desk.
///
/// **Declared here rather than in the host** so that a painter can name one
/// without reaching across the firewall, and converted on the other side by an
/// exhaustive match — which is what makes the two orders provably the same
/// rather than the same by inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strip {
    /// One instrument slot. **One strip each, loaded or not.**
    ///
    /// There used to be a single "instrument" channel for the sum of them,
    /// which was wrong the moment somebody loaded a second: a rack of five
    /// with one fader is not a mixer, it is a master with extra steps. An
    /// empty slot still gets a strip, drawn as an outline you can load into.
    Slot(usize),
    /// One input of the interface. **One strip each, chosen or not.**
    ///
    /// A rig is not one microphone. The owner's case is a vocal on input 6 and
    /// a synth across 4/5, live at the same time, and one lumped "input"
    /// channel for both is a master with extra steps — the same argument that
    /// gave every instrument slot a strip of its own.
    ///
    /// One INTERFACE, though. A second device is a second clock and is
    /// declined on purpose; anyone with that rig makes an aggregate device,
    /// which presents as one device and arrives here as the ordinary case.
    Input(usize),
    Track,
    Click,
    /// The return from the effects bus. It has no send of its own: a bus that
    /// could feed itself is a bus that howls.
    Fx,
}

/// How many inputs of one interface the desk has room for.
///
/// **Must equal `ivory_record::audio::MAX_PICKS`**, which is the same number
/// counted where the capture happens. This crate cannot reach that one — see
/// the firewall — so the host asserts they agree.
pub const INPUTS: usize = 4;

/// How many channels the desk has, master aside.
pub const STRIPS: usize = SLOTS + INPUTS + 3;

impl Strip {
    /// The channels the MIXER draws, in order.
    ///
    /// **Not all of them.** The click and the effects return have controls of
    /// their own everywhere else — the click's fader is in the band and the
    /// bus's three knobs are the band's — so a column each in the mixer was
    /// two columns of duplication and a narrower strip for everything that had
    /// nowhere else to be. They keep their place on the desk: the sends and
    /// the mute masks are unchanged, they are simply not drawn.
    pub fn shown() -> [Strip; SLOTS + INPUTS + 1] {
        std::array::from_fn(|i| {
            if i < SLOTS {
                Strip::Slot(i)
            } else if i < SLOTS + INPUTS {
                Strip::Input(i - SLOTS)
            } else {
                Strip::Track
            }
        })
    }

    /// Every strip, in the order they are drawn.
    pub fn all() -> [Strip; STRIPS] {
        std::array::from_fn(|i| {
            if i < SLOTS {
                Strip::Slot(i)
            } else if i < SLOTS + INPUTS {
                Strip::Input(i - SLOTS)
            } else {
                [Strip::Track, Strip::Click, Strip::Fx][i - SLOTS - INPUTS]
            }
        })
    }

    /// Its place in `Desk`'s arrays.
    pub const fn index(self) -> usize {
        match self {
            Strip::Slot(i) => i,
            Strip::Input(i) => SLOTS + i,
            Strip::Track => SLOTS + INPUTS,
            Strip::Click => SLOTS + INPUTS + 1,
            Strip::Fx => SLOTS + INPUTS + 2,
        }
    }

    /// Whether it can send to the effects bus. Everything but the bus itself.
    pub const fn sends(self) -> bool {
        !matches!(self, Strip::Fx)
    }

    /// What the strip is called where a sentence has to name it.
    ///
    /// Lower case and bare, because every caller so far puts it mid-sentence.
    /// A slot says which one, since "the instrument is muted" on a rack of
    /// five is not an instruction anybody can follow.
    pub fn label(self) -> String {
        match self {
            Strip::Slot(i) => format!("instrument {}", i + 1),
            // Numbered only when there could be more than one of them on the
            // desk, which there can be — "the input is muted" with two open is
            // not something anybody can act on.
            Strip::Input(i) => format!("input {}", i + 1),
            Strip::Track => "the backing track".to_owned(),
            Strip::Click => "the click".to_owned(),
            Strip::Fx => "the effects bus".to_owned(),
        }
    }
}

/// What to say about sources a take is going to be missing.
///
/// **The sentence lives here, not in the host**, for the reason every string
/// in this crate does: it is the part that can be tested without a device, a
/// window or a take. The host decides WHICH strips are lost — that needs an
/// engine and an open input — and this decides how to say it.
///
/// `None` for an empty list, so the caller can chain it straight into a status
/// line that is otherwise about live errors.
pub fn missing_from_take(lost: &[Strip]) -> Option<String> {
    let mut names: Vec<String> = lost.iter().map(|s| s.label()).collect();
    let many = names.len() > 1;
    let what = match names.len() {
        0 => return None,
        1 => names.remove(0),
        // "the input, instrument 2 and the backing track" — the list anybody
        // would say out loud, rather than a count of a number nobody can act
        // on.
        _ => {
            let last = names.pop()?;
            format!("{} and {last}", names.join(", "))
        }
    };
    let (is_are, it_them) = if many { ("are", "them") } else { ("is", "it") };
    Some(format!(
        "{what} {is_are} muted, so this take will not have {it_them} - the \
         mixer is on Tab"
    ))
}

/// The routing half of the desk: what goes to the effects, and what is heard.
///
/// **Levels are in [`Gains`] and not here**, because every one of them already
/// existed and is already pushed: the mixer's faders are the band's faders seen
/// a second time, which is the whole reason the band can stay exactly as it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Desk {
    /// How much of each strip goes to the effects bus, 0..=1. Indexed by
    /// [`Strip::index`]; the `Fx` slot is unused and stays zero.
    pub send: [f32; STRIPS],
    pub muted: [bool; STRIPS],
    pub soloed: [bool; STRIPS],
}

impl Default for Desk {
    /// **The routing this app had before it had a mixer.** The instrument sends
    /// everything, nothing else sends anything, nothing is muted and nothing is
    /// soloed — so a settings file written before any of this existed comes up
    /// sounding exactly as it did.
    fn default() -> Self {
        // **Every instrument sends everything, and nothing else sends
        // anything.** That is what an insert on the instrument bus was, so a
        // settings file written before any of this existed comes up sounding
        // exactly as it did.
        let mut send = [0.0; STRIPS];
        for i in 0..SLOTS {
            send[i] = 1.0;
        }
        Self {
            send,
            muted: [false; STRIPS],
            soloed: [false; STRIPS],
        }
    }
}

impl Desk {
    /// Whether anything is soloed, which is what makes solo exclusive.
    pub fn any_solo(&self) -> bool {
        self.soloed.iter().any(|s| *s)
    }

    /// Whether a strip is heard — which is now also whether it is RECORDED.
    ///
    /// The take is the desk (see `record::TakeSource`), so this one rule
    /// answers "is it in the file" as well as "is it in the room". The audio
    /// thread's `strip_is_heard` is the same rule over the bit masks this same
    /// `Desk` was pushed as; two answers to that question is the shape of the
    /// bug that collapsing `TakeSource` removed.
    ///
    /// Solo wins, and a soloed strip is heard even if it is also muted —
    /// pressing solo on a muted channel is a request to hear it, and a solo
    /// button that sometimes does nothing is worse than one that overrules.
    pub fn heard(&self, strip: Strip) -> bool {
        if self.any_solo() {
            return self.soloed[strip.index()];
        }
        !self.muted[strip.index()]
    }
}

impl Default for Gains {
    /// Unity for the sources, and the click **under** them.
    ///
    /// -6 dB on the metronome is not timidity: a click at the same level as a
    /// piano is all you can hear, and every one of these gets turned down by
    /// hand within a minute otherwise.
    fn default() -> Self {
        Self {
            slots: [1.0; SLOTS],
            metronome: 0.5,
            inputs: [1.0; INPUTS],
            master: 1.0,
            track: 1.0,
            fx_return: 1.0,
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
/// One input of the interface, as the desk needs to draw it.
///
/// **Filled by the host every frame, like the slots.** Which inputs are open
/// is a fact about the device and the picker, and `ivory-ui` cannot ask either
/// of them — see the firewall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    /// What the picker called it — "Scarlett 18i20  -  input 6". Empty for a
    /// strip nobody has filled, which is drawn as somewhere to put one.
    pub name: String,
    /// Two channels rather than one, which is what decides two meter bars.
    /// A mono microphone drawn as two identical bars is a meter claiming a
    /// stereo signal.
    pub stereo: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RecorderState {
    pub state: RecordState,
    pub elapsed_s: f64,
    /// The loudest thing each strip made since the last frame, in
    /// [`Strip::ALL`] order. Pushed by the host, read by the mixer.
    pub strip_peaks: [[f32; 2]; STRIPS],
    pub meters: Meters,
    /// The output's levels and the limiter's reduction. See the same two
    /// fields on [`RecorderView`].
    pub master: Meters,
    pub gr_db: f32,
    /// The output directory, shortened for display.
    pub dest: String,
    /// What the next take's folder will be called, from `ivory_record::take`.
    pub folder_preview: String,
    pub camera_name: Option<String>,
    /// The camera named in settings is not present right now.
    pub camera_missing: bool,
    /// The loaded instrument's display name.
    /// One per slot, filled by the host each frame.
    pub slots: [SlotState; SLOTS],
    /// The inputs of the interface that are open, one strip each.
    pub inputs: [InputState; INPUTS],
    pub audio_name: Option<String>,
    pub audio_missing: bool,
    pub preview: Option<Preview>,
    pub disk_minutes: Option<f64>,
    pub message: Option<String>,
    pub clip_warning: bool,
    /// The folder the last finished take went to, while it is still the thing
    /// on screen. `Some` is what makes Export mean "re-export that take"
    /// rather than "set up the next one".
    pub last_take_folder: Option<String>,
}

impl RecorderState {
    /// Borrow it as the painter wants it.
    ///
    /// The take name, the pre-roll and the hide-elapsed flag come from the app
    /// rather than from here: they are settings and edit state the app already
    /// owns, and a second copy would be a second thing to keep in step.
    /// What the camera is, as a label, without building a whole
    /// [`RecorderView`] to ask.
    ///
    /// The camera pane is drawn from the app's band layout now, which has no
    /// `RecorderView` in reach and no business making one — it needs one fact
    /// about the device, and this is it.
    pub fn camera_label(&self) -> DeviceLabel<'_> {
        match self.camera_name.as_deref() {
            None => DeviceLabel::None,
            Some(n) if self.camera_missing => DeviceLabel::Missing(n),
            Some(n) => DeviceLabel::Open(n),
        }
    }

    pub fn view<'a>(
        &'a self,
        take_name: &'a str,
        name_focused: bool,
        editing: Option<&'a NumEdit>,
        knobs: Knobs,
        hide_elapsed: bool,
        turning: Option<NumField>,
    ) -> RecorderView<'a> {
        let label = |name: &'a Option<String>, missing: bool| match name.as_deref() {
            None => DeviceLabel::None,
            Some(n) if missing => DeviceLabel::Missing(n),
            Some(n) => DeviceLabel::Open(n),
        };
        RecorderView {
            input_monitor: false,
            state: self.state,
            elapsed_s: self.elapsed_s,
            meters: self.meters,
            master: self.master,
            gr_db: self.gr_db,
            dest: &self.dest,
            take_name,
            name_focused,
            editing,
            folder_preview: &self.folder_preview,
            slots: std::array::from_fn(|i| SlotView {
                name: self.slots[i].name.as_deref(),
                missing: self.slots[i].missing,
                gain: knobs.gains.slots[i],
                has_editor: self.slots[i].has_editor,
                editor_open: self.slots[i].editor_open,
            }),
            gains: knobs.gains,
            metronome_on: knobs.metronome_on,
            metronome_in_take: knobs.metronome_in_take,
            tempo_bpm: knobs.tempo_bpm,
            count_in_beats: knobs.count_in_beats,
            count_in_bars: knobs.count_in_bars,
            time_signature: knobs.time_signature,
            fx: knobs.fx,
            // Nothing until the caller says otherwise, for the same reason as
            // the units below: the track is the host's to describe.
            track: crate::ports::TrackInfo::NONE,
            // Percent until the caller says otherwise: what each knob's number
            // MEANS is the host describing its own sweeps, not a preference,
            // so it is set over the top rather than threaded through here.
            fx_units: [crate::ports::KnobUnit::Percent; 6],
            turning,
            camera: label(&self.camera_name, self.camera_missing),
            audio: label(&self.audio_name, self.audio_missing),
            preview: self.preview,
            disk_minutes: self.disk_minutes,
            hide_elapsed,
            message: self.message.as_deref(),
            clip_warning: self.clip_warning,
        }
    }
}

/// The settings the band renders but does not own.
///
/// Where the six effect knobs are, 0..=1.
///
/// **One struct, because it was three loose floats in three places.** Adding
/// the filters and the limiter would have made that eighteen fields to keep in
/// step by hand, and a knob wired to the wrong one of them looks exactly like a
/// knob that does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FxSends {
    pub reverb: f32,
    pub delay: f32,
    pub chorus: f32,
    /// The high-pass corner. 0 is out of the way.
    pub hpf: f32,
    /// The low-pass corner. **Up is darker** — see the binary's `Sends::lpf`.
    pub lpf: f32,
    /// How hard the limiter is driven. 0 is bypass.
    pub limiter: f32,
}

impl FxSends {
    /// In the order they are drawn: the row of sends, then the row of
    /// dynamics. Used by anything that walks all six.
    pub fn get(&self, fx: crate::recorder_panel::Fx) -> f32 {
        use crate::recorder_panel::Fx;
        match fx {
            Fx::Reverb => self.reverb,
            Fx::Delay => self.delay,
            Fx::Chorus => self.chorus,
            Fx::Hpf => self.hpf,
            Fx::Lpf => self.lpf,
            Fx::Limiter => self.limiter,
        }
    }
}

/// Passed in rather than stored on [`RecorderState`] because every one of them
/// is a persisted user preference the app already holds, and a second copy is a
/// second thing to keep in step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    pub gains: Gains,
    pub metronome_on: bool,
    pub metronome_in_take: bool,
    pub tempo_bpm: f64,
    pub count_in_beats: u32,
    pub count_in_bars: u32,
    pub time_signature: TimeSignature,
    /// The six effect knobs, 0..=1. See `effects.rs` in the binary for what
    /// they reach: every instrument, and nothing else.
    pub fx: FxSends,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            gains: Gains::default(),
            metronome_on: false,
            metronome_in_take: false,
            tempo_bpm: DEFAULT_BPM,
            count_in_beats: 4,
            count_in_bars: 1,
            fx: FxSends::default(),
            time_signature: TimeSignature::default(),
        }
    }
}

/// The owned half of [`SlotView`].
#[derive(Debug, Clone, Default)]
pub struct SlotState {
    pub name: Option<String>,
    pub missing: bool,
    pub has_editor: bool,
    pub editor_open: bool,
}

/// Something the band asked the host to do, drained after the frame.
///
/// The **request pattern**, for the same reason the directory picker uses it:
/// creating a take directory and opening a device must not happen halfway
/// through painting a frame. The plugin refuses simply by never draining.
///
/// Deliberately short. Everything else a click in the band can do — choosing a
/// folder, choosing a device, changing the count-in, opening the Export dialog
/// — is either a settings write or a dialog, and both of those are the app's
/// own business. A request enum that carried them would be a second, weaker
/// copy of the menu. What is here is what needs a THING the app does not own: a
/// take, or a window belonging to somebody else's code.
// Not `Copy`: `Audition` carries the notes it wants heard, and a list of them
// is the only honest shape for a chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderRequest {
    /// The one button. Start a pre-roll, start a take, or stop one — the
    /// session decides which, because only it knows what state it is in.
    Toggle,
    /// Stop, unconditionally. Separate from `Toggle` because the band draws a
    /// distinct Stop control and "the button that stops it" must not also be
    /// able to start one.
    Stop,
    /// Put out every clip latch: the user has seen it.
    DismissClip,
    /// Open slot `n`'s OWN editor — the plugin's window, with its presets and
    /// its knobs.
    ///
    /// A request rather than a dialog, because the window is not ours: the
    /// plugin draws into a native window the host creates after the frame. VST3
    /// requires it on the main thread, and creating an AppKit window with an
    /// egui frame still on the stack is the same re-entrancy the folder picker
    /// avoids.
    OpenPluginEditor(usize),
    /// Open the patch editor on what the built-in is playing: the host
    /// answers with `IvoryApp::set_patch_edit`.
    EditPatch {
        slot: usize,
    },
    /// One row of the patch moved. See `dx7::edit` for what the address means.
    SetPatchParam {
        group: usize,
        index: usize,
        value: i32,
    },
    SetPatchName(String),
    /// Write the patch being edited into the user's own bank.
    SavePatch,
    /// Play patch `index` of the loaded cartridge in `slot`'s built-in.
    /// `usize::MAX` means the patch compiled into the app.
    ChoosePatch {
        slot: usize,
        index: usize,
    },
    /// **Sound these notes through the loaded instrument.**
    ///
    /// The one way this crate can make a noise. It cannot reach the engine —
    /// the firewall is the whole point — so it says what it wants heard and
    /// the host plays it.
    ///
    /// **A held gesture, not a timed one.** `on` is the edge: true when the
    /// key or the mouse goes down, false when it comes up. A duration was
    /// tried first and was wrong twice over. A note that stops on a timer
    /// stops in the middle of a phrase somebody is still holding; and
    /// scheduling the note-off far in the future put it in the MIDI queue
    /// ahead of everything else, where it blocked the drain and delivered the
    /// second, third and fourth notes of a chord a second and a half after the
    /// first.
    ///
    /// The release is guaranteed by the sender, not by a timer: losing focus
    /// counts as letting go, so there is no state in which the key is up and
    /// the note is still sounding.
    Audition { notes: Vec<u8>, on: bool },
}

/// How hard an auditioned note is struck.
///
/// Firmly, but not at the top of the scale: a sampled piano's loudest layer is
/// usually its harshest, and this is a note somebody asked to hear rather than
/// one they played.
pub const AUDITION_VELOCITY: u8 = 88;

// ── The export contract ─────────────────────────────────────────────────────

/// How many video files a take produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoMode {
    /// Audio and MIDI only.
    None,
    /// One file containing whichever of camera / display / audio are ticked.
    ///
    /// **The default, and it does not need a camera.** A take records the
    /// WINDOW — the piano, the chord, the diagrams, the band — and the camera
    /// is an inset when there is one. This was `None` on the theory that
    /// somebody recording a practice session wants audio and MIDI only; what
    /// actually happened is that a tester recorded a take, got no `.mp4`, and
    /// had no way to know video was a thing they had to go and switch on.
    ///
    /// A take that was supposed to produce a video and did not is the failure
    /// that wastes a performance, and it costs a rerun to find out.
    #[default]
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
    /// **The display fills the frame; the camera is a small inset over it.**
    ///
    /// The default, and the opposite of what it was. `CameraAbove` was the
    /// default on the reading that this is a tutorial video — hands above,
    /// keys below — and that is a real shape, but it makes the app the
    /// SECONDARY thing: a fretboard and three theory diagrams squeezed into a
    /// band under a webcam are too small to read, which is the whole reason
    /// anybody would record them.
    ///
    /// This app draws the thing worth watching. The camera is the context.
    #[default]
    DisplayFull,
    /// Camera on top, the display below. The shape of every piano tutorial
    /// video ever made.
    CameraAbove,
    DisplayAbove,
    /// Camera fills the frame with the display floated over the bottom of it.
    CameraFull,
    SideBySide,
}

impl Layout {
    pub const ALL: [Layout; 5] = [
        Layout::DisplayFull,
        Layout::CameraAbove,
        Layout::DisplayAbove,
        Layout::CameraFull,
        Layout::SideBySide,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Layout::DisplayFull => "Full app, camera inset",
            Layout::CameraAbove => "Camera above, display below",
            Layout::DisplayAbove => "Display above, camera below",
            Layout::CameraFull => "Camera full frame, display overlaid",
            Layout::SideBySide => "Side by side",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Layout::DisplayFull => "display_full",
            Layout::CameraAbove => "camera_above",
            Layout::DisplayAbove => "display_above",
            Layout::CameraFull => "camera_full",
            Layout::SideBySide => "side_by_side",
        }
    }

    fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.key() == k)
    }

    /// Where the camera and the display go inside one video frame.
    ///
    /// **The display gets a BAND, not a half.** This is the one decision in the
    /// whole layout that is worth arguing about, so: the display content is a
    /// piano keyboard and a chord name, which is a very wide and very short
    /// thing. Splitting a 16:9 frame down the middle gives it twice the height
    /// it can use and starves the camera of the room that shows a pianist's
    /// hands. So in landscape it takes roughly a third, and the camera takes
    /// the rest.
    ///
    /// **Portrait is the exception, and it is why 9:16 is worth offering.**
    /// A vertical frame has height to spare, so the keyboard can have 40% of it
    /// and still leave the camera a near-square pane — which is a better crop of
    /// a person at a piano than 16:9 ever gives. In 9:16 the stack is not a
    /// compromise, it is the best arrangement available.
    ///
    /// Returns `(camera, display)`. Either is `None` when that layer is not in
    /// the composite; when only one is, it takes the whole frame, because a
    /// letterbox around the only thing in the video is a bug rather than a
    /// layout.
    ///
    /// **Each layer arrives as `Some(aspect)` or `None`.** It used to be two
    /// bools and one float, and adding the camera's aspect would have made it
    /// two bools and two adjacent floats of different meaning — the exact shape
    /// that gets passed in the wrong order once and then silently lays out
    /// every video slightly wrong. Presence and proportion are one fact per
    /// layer, so they travel as one value.
    pub fn split(
        self,
        frame: egui::Rect,
        camera: Option<f32>,
        display: Option<f32>,
    ) -> Panes {
        let portrait = frame.height() > frame.width();
        match (camera, display) {
            (None, None) => Panes::default(),
            (Some(_), None) => Panes {
                camera: Some(frame),
                display: None,
            },
            (None, Some(_)) => Panes {
                camera: None,
                display: Some(frame),
            },
            (Some(cam), Some(disp)) => self.both(frame, portrait, disp, cam),
        }
    }

    /// How tall the `DisplayFull` inset is, as a fraction of the frame's short
    /// edge.
    ///
    /// **Smaller than it looks.** At 16:9 this is a sixth of the height and
    /// about a tenth of the width — a person in the corner of a screen, which
    /// is the whole brief. It started at a quarter of the WIDTH, which put a
    /// webcam over a sixth of the picture.
    const INSET_H: f32 = 0.17;

    /// What an inset is assumed to be shaped like when nothing has said.
    ///
    /// A webcam that has not delivered a frame yet has no size, and the inset
    /// still has to be somewhere. 16:9 is what almost every camera on a desk
    /// is, and it is the shape of the frame it sits in.
    pub const DEFAULT_CAMERA_ASPECT: f32 = 16.0 / 9.0;

    /// How tall the display band should be inside a frame this wide.
    ///
    /// **Its content's own height, not a fixed fraction.** The first version
    /// gave the band a flat 30% of a landscape frame and 40% of a portrait one,
    /// and the very first composited frame showed why that is wrong: in a
    /// 360x640 vertical frame the keyboard is 55 points tall and it was handed
    /// 256, so five sixths of the band was dead black and the camera lost a
    /// quarter of the picture to hold it.
    ///
    /// The cap is still there, because the band grows with every panel switched
    /// on and a fretboard plus three theory diagrams could otherwise take the
    /// whole frame.
    fn band_height(frame: egui::Rect, portrait: bool, display_aspect: f32) -> f32 {
        let natural = if display_aspect > 0.01 {
            frame.width() / display_aspect
        } else {
            frame.height() * 0.30
        };
        let cap = frame.height() * if portrait { 0.45 } else { 0.40 };
        natural.clamp(0.0, cap)
    }

    fn both(
        self,
        frame: egui::Rect,
        portrait: bool,
        display_aspect: f32,
        camera_aspect: f32,
    ) -> Panes {
        let band = Self::band_height(frame, portrait, display_aspect) / frame.height().max(1.0);
        match self {
            // The display gets the whole frame and the camera sits in a corner
            // of it. A QUARTER of the width, which is the size a face has to be
            // to read as a person and no larger — the point of this layout is
            // that the app is what you are watching.
            //
            // Bottom RIGHT: the app's own bands run full width and the busiest
            // of them is the keyboard along the bottom, but the right-hand end
            // of it is the top octaves, which are the least played. Top-right
            // would sit over the chord name.
            Layout::DisplayFull => {
                // **The camera's own shape, sized off the short edge, top
                // right.**
                //
                // Its own shape because that is the only size at which nothing
                // is thrown away: `paint_camera` centre-crops to whatever pane
                // it is given, so a square inset silently binned the sides of
                // every 16:9 webcam, and a 16:9 inset would do the same to a
                // 4:3 one. The pane follows the sensor and the whole picture
                // survives.
                //
                // Off the SHORT edge, so it is the same apparent size in a 9:16
                // reel as in a 16:9 video — a fraction of the width would be a
                // quarter of one frame and a postage stamp in the other. And
                // capped at a third of the width so that a very wide camera
                // cannot walk across the picture the layout exists to show.
                //
                // Top right rather than bottom right: the busiest thing in the
                // app is the keyboard along the bottom with the fretboard under
                // it, and both run full width. The top right corner is the end
                // of the theory band, which is the one region with air in it.
                let short = frame.width().min(frame.height());
                let h = short * Self::INSET_H;
                let w = (h * camera_aspect.max(0.05)).min(frame.width() * 0.33);
                let margin = short * 0.025;
                Panes {
                    camera: Some(egui::Rect::from_min_max(
                        egui::Pos2::new(frame.right() - margin - w, frame.top() + margin),
                        egui::Pos2::new(frame.right() - margin, frame.top() + margin + h),
                    )),
                    display: Some(frame),
                }
            }
            Layout::CameraAbove => {
                let cut = frame.bottom() - frame.height() * band;
                Panes {
                    camera: Some(above(frame, cut)),
                    display: Some(below(frame, cut)),
                }
            }
            Layout::DisplayAbove => {
                let cut = frame.top() + frame.height() * band;
                Panes {
                    camera: Some(below(frame, cut)),
                    display: Some(above(frame, cut)),
                }
            }
            // Overlaid, so the camera keeps the whole frame and the display
            // floats over the bottom of it. A little shorter than the stacked
            // band because it is covering the picture rather than sitting
            // beside it.
            Layout::CameraFull => {
                // Overlaid, so it sits a little tighter than the stacked band:
                // it is covering the picture rather than sitting beside it, and
                // an overlay is a caption, not a second pane.
                let h = Self::band_height(frame, portrait, display_aspect * 1.15);
                Panes {
                    camera: Some(frame),
                    display: Some(below(frame, frame.bottom() - h)),
                }
            }
            Layout::SideBySide => {
                // **In portrait this stacks instead**, and that is deliberate
                // rather than a limitation. Two halves of a 9:16 frame are two
                // 9:32 slivers: unusable for a face and unusable for a
                // keyboard. Silently giving the user the arrangement that works
                // is better than giving them the one they literally asked for
                // and letting them find out at the export.
                if portrait {
                    return Layout::CameraAbove.both(frame, portrait, display_aspect, camera_aspect);
                }
                let cut = frame.left() + frame.width() * 0.5;
                Panes {
                    camera: Some(egui::Rect::from_min_max(
                        frame.min,
                        egui::Pos2::new(cut, frame.bottom()),
                    )),
                    display: Some(egui::Rect::from_min_max(
                        egui::Pos2::new(cut, frame.top()),
                        frame.max,
                    )),
                }
            }
        }
    }

    /// Whether the display is drawn ON TOP of the camera rather than beside it.
    ///
    /// The compositor needs to know: an overlaid band is painted after the
    /// camera and over it, and a stacked one is painted into a region the
    /// camera never touches.
    pub fn overlays(self) -> bool {
        matches!(self, Layout::CameraFull | Layout::DisplayFull)
    }

    /// Whether the CAMERA is the thing floated on top rather than the display.
    ///
    /// The compositor paints the overlaid layer last, and which one that is
    /// differs: `CameraFull` floats the display over the camera, `DisplayFull`
    /// floats the camera over the display.
    pub fn camera_on_top(self) -> bool {
        matches!(self, Layout::DisplayFull)
    }
}

/// Where each layer goes in the composited frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Panes {
    pub camera: Option<egui::Rect>,
    pub display: Option<egui::Rect>,
}

fn above(frame: egui::Rect, cut: f32) -> egui::Rect {
    egui::Rect::from_min_max(frame.min, egui::Pos2::new(frame.right(), cut))
}

fn below(frame: egui::Rect, cut: f32) -> egui::Rect {
    egui::Rect::from_min_max(egui::Pos2::new(frame.left(), cut), frame.max)
}

/// The composited video's frame size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    #[default]
    Hd1080,
    Hd720,
    /// **9:16, for Reels / Shorts / TikTok.**
    ///
    /// Not a crop of the landscape frame — a frame shape of its own, which the
    /// layouts read and lay out differently. It is the one aspect where the
    /// stacked layout is not a compromise but the best possible arrangement:
    /// camera above and keyboard below, both full width, no wasted margin
    /// anywhere. In 16:9 the same stack has to give the keyboard a band; in
    /// 9:16 there is height to give it.
    Vertical1080,
    /// Whatever the camera is giving us. Sharpest, and the one that makes the
    /// file size unpredictable, which is why it is not the default.
    MatchCamera,
}

impl Resolution {
    pub const ALL: [Resolution; 4] = [
        Resolution::Hd1080,
        Resolution::Hd720,
        Resolution::Vertical1080,
        Resolution::MatchCamera,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Resolution::Hd1080 => "1920x1080",
            Resolution::Hd720 => "1280x720",
            // Named for what it is FOR. "1080x1920" is a number nobody
            // recognises; "Reels / Shorts" is the reason somebody is choosing
            // it, and the number is there for the one person who wants it.
            Resolution::Vertical1080 => "1080x1920  (Reels / Shorts)",
            Resolution::MatchCamera => "Match camera",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Resolution::Hd1080 => "1080",
            Resolution::Hd720 => "720",
            Resolution::Vertical1080 => "vertical",
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
            Resolution::Vertical1080 => Some((1080, 1920)),
            Resolution::MatchCamera => None,
        }
    }

    /// Whether the frame is taller than it is wide.
    ///
    /// The layouts read this rather than measuring the rectangle, so that a
    /// `MatchCamera` frame from a phone held upright is treated as the portrait
    /// frame it is.
    pub fn is_portrait(self, camera: Option<(u32, u32)>) -> bool {
        match self.pixels().or(camera) {
            Some((w, h)) => h > w,
            None => false,
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
    /// **Everything.**
    ///
    /// It was piano and chord name, on the reasoning that those are the two
    /// panels about what was just played. That reasoning was fine and the
    /// result was wrong: somebody who has turned every band on, and can see
    /// them, presses Record and gets a video with two of them in it — and
    /// nothing on screen ever said the video kept its own list.
    ///
    /// A video with a panel you did not want is a tick away from being fixed.
    /// A video missing the panel you play from is a take you cannot get back.
    fn default() -> Self {
        Self {
            piano: true,
            chord: true,
            fretboard: true,
            theory: true,
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

    /// Whether this spec actually writes a video file, given the camera.
    ///
    /// The host asks before opening an encoder, and the answer has to be the
    /// same one `begin_video` acts on — this is that decision, not a second
    /// copy of it. **A camera is not required.** A take with no camera at all
    /// still records the window, which is the part somebody plays from; a
    /// tester recorded a whole session, had no webcam, and got no `.mp4`,
    /// because the only thing that had ever switched video on was picking a
    /// camera.
    pub fn produces_video(&self, camera_running: bool) -> bool {
        if !self.video.wants_video() {
            return false;
        }
        let camera = self.composite.camera && camera_running;
        let display = self.composite.display && self.composite.shows.any();
        camera || display
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
            // Vertical 1080 is the same pixel count as landscape 1080 turned on
            // its side, so it costs the same. Listed separately rather than
            // folded in with Hd1080 because they are not the same frame and a
            // reader checking this table should see that they were both
            // considered.
            Resolution::Vertical1080 => 12.0 * 60.0 / 8.0,
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
/// **15 is on this list for machines that render video on the CPU.**
///
/// A 2012-era integrated GPU has no Vulkan driver, so mesa's lavapipe
/// rasterises every composited frame on the same cores running the audio
/// callback and the encoder. At 1080p30 the owner's Linux box delivered 44% of
/// its frames and the app was unplayable while it filmed. A 15 fps video of a
/// good take beats a 30 fps video of an unplayable one.
pub const FPS_CHOICES: [u32; 4] = [15, 24, 30, 60];

/// Count-in choices, in BARS.
///
/// Bars now that there IS a time signature to count them in. It used to be
/// beats — 0, 4, 8 — with "(2 bars of 4)" in brackets and 4/4 assumed and
/// stated, because inventing a signature to describe a click would have been a
/// bigger lie than counting beats. In 6/8 that assumption stops being a
/// simplification and starts being wrong: two bars is twelve clicks, and no
/// number of beats is the right answer at every signature.
pub const COUNT_IN_CHOICES: [u32; 4] = [0, 1, 2, 4];

/// Gain range for every fader in the band, in dB, plus what 1.0 means.
///
/// A linear 0..=1 fader is unusable for audio — the whole useful range of a
/// monitor level is squeezed into the top fifth of the travel. These are the
/// endpoints the band's faders map onto.
pub const GAIN_MIN_DB: f32 = -60.0;
pub const GAIN_MAX_DB: f32 = 12.0;

/// A fader position (0..=1, left to right) as a linear gain.
///
/// Below the bottom of the scale is silence rather than -60 dB: a fader pulled
/// all the way down has to be OFF, and 0.001 is not off when the thing it is
/// attenuating is a piano recorded at full scale.
pub fn fader_to_gain(position: f32) -> f32 {
    let p = position.clamp(0.0, 1.0);
    if p <= 0.0 {
        return 0.0;
    }
    let db = GAIN_MIN_DB + p * (GAIN_MAX_DB - GAIN_MIN_DB);
    10f32.powf(db / 20.0)
}

/// The inverse, for drawing a stored gain back as a fader position.
pub fn gain_to_fader(gain: f32) -> f32 {
    if gain <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * gain.log10();
    ((db - GAIN_MIN_DB) / (GAIN_MAX_DB - GAIN_MIN_DB)).clamp(0.0, 1.0)
}

/// "-6.0 dB", "0.0 dB", "OFF" — what the number beside a fader says.
pub fn gain_text(gain: f32) -> String {
    if gain <= 0.0 {
        return "OFF".to_owned();
    }
    format!("{:+.1} dB", 20.0 * gain.log10())
}

// ── Typing a number in ──────────────────────────────────────────────────────
//
// Every control in the band that shows a number can be typed into as well as
// dragged, because a drag cannot hit 120 BPM on a band fifteen points high and
// "about 118" is not a tempo anybody wants in a MIDI file. The gesture is a
// TAP: press and release without moving opens the field, press and move is the
// drag it always was. Nothing had to be given up for it — the only thing a tap
// used to do was jump the value to wherever the cursor happened to be, which is
// the least precise thing either gesture can do.

/// A control in the band that carries a number, and so can be typed into.
///
/// Deliberately NOT the `Hit` itself: a `Hit` carries the value the gesture
/// would set, so two frames of the same drag are two different `Hit`s, and the
/// identity of the field being edited has to outlive that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NumField {
    Slot(usize),
    Metronome,
    Input,
    /// The master, typed in **decibels** like the faders it shares a curve
    /// with — not a percentage. It is a level.
    Master,
    /// The backing track's level, in decibels like the faders beside it.
    Track,
    /// The backing track's trim points, typed as `m:ss.t` or as seconds.
    TrackIn,
    TrackOut,
    /// One of the six effect knobs, typed as a PERCENT. Every other field
    /// here is typed in the unit it is displayed in, and "40" for four tenths
    /// wet is the only reading of a send anybody has ever wanted to write.
    Fx(crate::recorder_panel::Fx),
    Tempo,
    /// The time signature. Typed rather than dragged — "6/8" is two numbers and
    /// a slash, and there is no continuum between 4/4 and 7/8 to drag along.
    Meter,
}

/// A time typed into a trim field, in seconds.
///
/// **Both ways somebody writes one.** `12.5` is twelve and a half seconds and
/// `1:12.5` is a minute and twelve — a backing track is minutes long, so the
/// second is what anybody reading a player's display will type, and refusing
/// it would be a field that rejects the format it prints.
pub fn parse_time(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let (mins, rest) = match t.split_once(':') {
        Some((m, r)) => (m.trim().parse::<f64>().ok()?, r.trim()),
        None => (0.0, t),
    };
    let secs = rest.parse::<f64>().ok()?;
    if !mins.is_finite() || !secs.is_finite() || mins < 0.0 || secs < 0.0 {
        return None;
    }
    Some(mins * 60.0 + secs)
}

/// A percent typed into an effect send, as 0..=1.
///
/// Lenient about a trailing `%` because somebody typing a percentage will write
/// one, and refuses anything that is not a number rather than treating it as
/// zero — a field that silently reads "abc" as silence is a field that loses
/// your setting when you fumble a key.
pub fn parse_percent(text: &str) -> Option<f32> {
    let t = text.trim().trim_end_matches('%').trim();
    let v: f32 = t.parse().ok()?;
    v.is_finite().then(|| (v / 100.0).clamp(0.0, 1.0))
}

/// A numeric field mid-edit: which one, and what has been typed so far.
///
/// The text is kept as TEXT and not parsed on every keystroke, because a field
/// passes through "-" and "12." on the way to "-12.5" and neither of those is a
/// number. Nothing is applied until it is committed.
#[derive(Clone, Debug)]
pub struct NumEdit {
    pub field: NumField,
    pub text: String,
}

impl NumEdit {
    /// Start editing `field`, seeded EMPTY rather than with the current value.
    ///
    /// Empty because typing over a selected value is what every DAW does and
    /// what the fingers expect: the first digit replaces, it does not append.
    /// Seeding with "+0.0 dB" would mean deleting seven characters before the
    /// first useful one.
    pub fn new(field: NumField) -> Self {
        Self {
            field,
            text: String::new(),
        }
    }

    /// Accept a typed character, or ignore it.
    ///
    /// Silently drops anything that cannot be part of a number so that a
    /// stray letter — the app's own single-key shortcuts, typed at a field
    /// that has focus — cannot end up in the box.
    pub fn push(&mut self, ch: char) {
        const MAX: usize = 8;
        // A signature is two numbers and a slash, and nothing else: no minus,
        // no decimal point. Handled first so those two cannot creep in.
        if self.field == NumField::Meter {
            let ok = ch.is_ascii_digit() || (ch == '/' && !self.text.contains('/'));
            if ok && self.text.len() < MAX {
                self.text.push(ch);
            }
            return;
        }
        let ok = ch.is_ascii_digit()
            // One dot, and a minus only in front. Neither check is about
            // rejecting bad input for its own sake: `.parse()` will do that
            // anyway. They stop the field from showing something it will then
            // silently refuse to accept.
            || (ch == '.' && !self.text.contains('.'))
            || (ch == '-' && self.text.is_empty());
        if ok && self.text.len() < MAX {
            self.text.push(ch);
        }
    }

    pub fn pop(&mut self) {
        self.text.pop();
    }
}

/// Turn typed text into a LINEAR gain, or `None` if it is not a level.
///
/// Reads dB, which is what the field displays. Blank commits nothing.
pub fn parse_gain(text: &str) -> Option<f32> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let db: f32 = t.parse().ok()?;
    // Below the bottom of the fader is OFF, not a very quiet signal. This is
    // the only way to type OFF, and it is the one people reach for: -60 and
    // "as low as it goes" are the same intention.
    if db <= GAIN_MIN_DB {
        return Some(0.0);
    }
    Some(10f32.powf(db.min(GAIN_MAX_DB) / 20.0))
}

/// Turn typed text into a tempo, clamped to what the SMF writer can express.
pub fn parse_bpm(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let bpm: f64 = t.parse().ok()?;
    if !bpm.is_finite() {
        return None;
    }
    Some(bpm.clamp(MIN_BPM, MAX_BPM))
}

/// A time signature, as a musician writes it: `beats`/`unit`.
///
/// # What the tempo means, and why it is not negotiable
///
/// **`tempo_bpm` counts QUARTER notes**, always, whatever the unit is. That is
/// not a preference — an SMF tempo meta event is microseconds per quarter note
/// and nothing else, so any other reading would make the `.mid` disagree with
/// the click in every bar. In 6/8 at 120 a quarter is half a second, so an
/// eighth is a quarter of a second and a bar of six is a second and a half.
///
/// It is worth being loud about because the other convention is common: many
/// musicians set 6/8 by its dotted-quarter pulse and would expect 120 to mean
/// two of those per bar. Under this reading that is 6/8 at 360, which the range
/// allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    /// Beats in a bar — the top number. 1..=32.
    pub beats: u8,
    /// What gets the beat — the bottom number. A power of two, 1..=32.
    pub unit: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self { beats: 4, unit: 4 }
    }
}

/// The ones worth putting in a menu, in the order a musician would look for
/// them. Anything else can still be typed — see [`TimeSignature::parse`].
pub const TIME_SIGNATURES: [TimeSignature; 8] = [
    TimeSignature { beats: 4, unit: 4 },
    TimeSignature { beats: 3, unit: 4 },
    TimeSignature { beats: 2, unit: 4 },
    TimeSignature { beats: 6, unit: 8 },
    TimeSignature { beats: 9, unit: 8 },
    TimeSignature { beats: 12, unit: 8 },
    TimeSignature { beats: 5, unit: 4 },
    TimeSignature { beats: 7, unit: 8 },
];

impl TimeSignature {
    /// Whether this is a signature anything can play or write.
    ///
    /// The unit has to be a power of two because that is what a note value IS,
    /// and because the SMF meta event stores it as a power: 8 is written as 3.
    pub fn is_valid(self) -> bool {
        (1..=32).contains(&self.beats) && matches!(self.unit, 1 | 2 | 4 | 8 | 16 | 32)
    }

    /// How long one beat of THIS signature lasts, in seconds.
    ///
    /// `4.0 / unit` is the whole of it: the tempo counts quarters, so an eighth
    /// is half a quarter and a half-note is two of them.
    pub fn beat_seconds(self, tempo_bpm: f64) -> f64 {
        let bpm = if tempo_bpm.is_finite() {
            tempo_bpm.clamp(MIN_BPM, MAX_BPM)
        } else {
            DEFAULT_BPM
        };
        (4.0 / f64::from(self.unit.max(1))) * (60.0 / bpm)
    }

    /// Beats in `bars` bars of this signature.
    pub fn beats_in(self, bars: u32) -> u32 {
        u32::from(self.beats.max(1)) * bars
    }

    /// The SMF meta event's second byte: the unit as a POWER of two.
    ///
    /// 4 is written as 2, 8 as 3. Getting this wrong writes a file every DAW
    /// reads as a different signature, which is the sort of thing nobody
    /// notices until a bar line is in the wrong place.
    pub fn unit_power(self) -> u8 {
        match self.unit {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            16 => 4,
            32 => 5,
            // Unreachable through `is_valid`; 4/4 is the answer that cannot
            // make a file unreadable.
            _ => 2,
        }
    }

    /// `"6/8"`, for a menu and for the settings file alike.
    pub fn label(self) -> String {
        format!("{}/{}", self.beats, self.unit)
    }

    /// The inverse. Anything that is not two numbers around a slash, or is not
    /// a signature, is `None` rather than a guess.
    pub fn parse(text: &str) -> Option<Self> {
        let (a, b) = text.trim().split_once('/')?;
        let sig = Self {
            beats: a.trim().parse().ok()?,
            unit: b.trim().parse().ok()?,
        };
        sig.is_valid().then_some(sig)
    }
}

/// One audio device, as the status panel reports it.
///
/// Plain data, filled by the binary: `ivory-ui` cannot see a device and must
/// not learn how. `None` throughout means "no device open", which is a real
/// state now that an input is never opened until one is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamStats {
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames per callback, when the device is running one it will admit to.
    /// `None` is cpal's `BufferSize::Default` — the device chose, and did not
    /// say what it chose.
    pub buffer_frames: Option<u32>,
}

impl StreamStats {
    /// One buffer's worth of time, in milliseconds.
    pub fn buffer_ms(self) -> Option<f64> {
        let frames = self.buffer_frames?;
        (self.sample_rate > 0).then(|| f64::from(frames) * 1000.0 / f64::from(self.sample_rate))
    }
}

/// What the status panel shows: both sides of the audio path.
#[derive(Debug, Clone, Default)]
pub struct AudioStatus {
    /// The input, when one is open.
    pub input: Option<(String, StreamStats)>,
    /// The monitor output, when the engine is running.
    pub output: Option<(String, StreamStats)>,
}

impl AudioStatus {
    /// **An ESTIMATE of round-trip latency, and labelled as one everywhere it
    /// is shown.**
    ///
    /// It is one buffer in plus one buffer out, which is the part anybody can
    /// compute. It is not the whole truth: a converter's own analogue-to-digital
    /// and digital-to-analogue stages, the USB frame, and whatever the driver
    /// does in between are all real and none of them are reported by cpal on
    /// any platform. So this is a floor, not a measurement — the true figure is
    /// always larger, typically by a few milliseconds.
    ///
    /// Measuring it properly means a loopback: play a click, record it, count
    /// the samples between. That is worth doing and it is not this.
    pub fn round_trip_ms(&self) -> Option<f64> {
        let a = self.input.as_ref()?.1.buffer_ms()?;
        let b = self.output.as_ref()?.1.buffer_ms()?;
        Some(a + b)
    }

    /// Whether the two sides disagree about the sample rate.
    ///
    /// Worth its own question because it is not cosmetic: the writer drains the
    /// instrument's ring at the INPUT's rate while the engine fills it at the
    /// OUTPUT's, so a mismatch overflows that ring and reports the losses
    /// against the take. Every device on the owner's machine is at 48k, which
    /// is exactly why this needs to be visible rather than discovered.
    pub fn rates_disagree(&self) -> bool {
        match (&self.input, &self.output) {
            (Some((_, a)), Some((_, b))) => {
                a.sample_rate > 0 && b.sample_rate > 0 && a.sample_rate != b.sample_rate
            }
            _ => false,
        }
    }
}

/// Buffer sizes the status panel offers, in frames.
///
/// Powers of two, because that is what every driver actually honours, and
/// stopping at 2048 because beyond it the latency is worse than the dropout it
/// was bought to prevent. `None` — the device's own default — is offered
/// alongside these and is what the app has always used.
pub const BUFFER_CHOICES: [u32; 6] = [64, 128, 256, 512, 1024, 2048];

/// Sample rates Setup offers, in Hz.
///
/// The six every interface names, and no more: this is a list to pick a
/// familiar number off, not a survey of what a driver will accept. What the
/// panel actually shows is this list intersected with what the INPUT device
/// reports — see `audio::input_rates` — because a rate offered and then
/// refused is a "could not open" every time somebody tries the biggest number.
///
/// `None`, the device's own, is offered alongside these and is what the app has
/// always used.
pub const SAMPLE_RATE_CHOICES: [u32; 6] =
    [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Tempo the metronome and the SMF tempo mark share.
///
/// One number, deliberately: a click at 90 against a file that says 120 is a
/// take nobody can edit afterwards.
pub const DEFAULT_BPM: f64 = 120.0;

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

    /// The bug this exists to stop coming back: record a take on a machine
    /// with no webcam, get a `.wav` and a `.mid` and no video, and have
    /// nothing anywhere explain that video was a setting.
    #[test]
    fn a_take_with_no_camera_still_writes_a_video() {
        assert!(
            ExportSpec::default().produces_video(false),
            "the window is the take"
        );
        assert!(ExportSpec::default().produces_video(true));

        // Off is still off, and a composite with nothing in it still writes
        // nothing — that is the case `begin_video` reports as an error.
        assert!(!ExportSpec {
            video: VideoMode::None,
            ..Default::default()
        }
        .produces_video(true));
        assert!(!ExportSpec {
            composite: Composite {
                camera: false,
                display: false,
                ..Default::default()
            },
            ..Default::default()
        }
        .produces_video(true));

        // Camera-only, no camera: nothing to encode.
        let cam_only = ExportSpec {
            composite: Composite {
                display: false,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(cam_only.produces_video(true));
        assert!(!cam_only.produces_video(false));
    }

    /// The camera question decides what a *finished* take can be re-exported
    /// as, so getting it wrong offers the user an option that cannot work.
    #[test]
    fn needing_a_camera_follows_the_video_mode_and_the_tick() {
        let no_cam = Composite {
            camera: false,
            ..Default::default()
        };
        assert!(!ExportSpec {
            video: VideoMode::None,
            ..Default::default()
        }
        .needs_camera());
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

    /// The count-in is not writing. If it were, every take would open with two
    /// bars of clicking.
    #[test]
    fn the_count_in_is_active_but_not_writing() {
        let p = RecordState::CountIn { beat: 2, of: 4 };
        assert!(p.is_active() && !p.is_writing());
        assert!(RecordState::Rolling.is_writing());
        assert!(RecordState::Finishing.is_writing());
        assert!(!RecordState::Idle.is_active());
    }
}

#[cfg(test)]
mod fader_tests {
    use super::*;

    /// A fader pulled all the way down has to be OFF. `-60 dB` is not off when
    /// what it is attenuating is a piano recorded at full scale — it is quiet,
    /// audible, and on the recording.
    #[test]
    fn the_bottom_of_a_fader_is_silence_and_not_merely_quiet() {
        assert_eq!(fader_to_gain(0.0), 0.0);
        assert_eq!(fader_to_gain(-1.0), 0.0, "and below the bottom, too");
        assert_eq!(gain_text(0.0), "OFF");
    }

    #[test]
    fn a_fader_round_trips_through_its_position() {
        for p in [0.05_f32, 0.25, 0.5, 0.8333333, 1.0] {
            let back = gain_to_fader(fader_to_gain(p));
            assert!(
                (back - p).abs() < 1e-4,
                "position {p} came back as {back}"
            );
        }
    }

    /// The one position anybody actually looks for.
    #[test]
    fn unity_gain_is_reachable_and_reads_as_zero_db() {
        let unity = gain_to_fader(1.0);
        assert!((fader_to_gain(unity) - 1.0).abs() < 1e-4);
        assert_eq!(gain_text(1.0), "+0.0 dB");
        assert!(
            unity > 0.0 && unity < 1.0,
            "0 dB has to be somewhere on the travel, not at an end"
        );
    }

    #[test]
    fn a_gain_above_unity_is_reachable_because_quiet_sources_exist() {
        assert!(fader_to_gain(1.0) > 1.0, "the scale runs past 0 dB");
        assert_eq!(gain_text(fader_to_gain(1.0)), "+12.0 dB");
    }

    /// The default that ruins takes if it is wrong.
    #[test]
    fn the_click_starts_under_the_music_and_out_of_the_file() {
        let k = Knobs::default();
        assert!(
            k.gains.slots.iter().all(|g| k.gains.metronome < *g),
            "the click starts under EVERY instrument, not just the first"
        );
        assert!(!k.metronome_in_take, "a click in the file is a ruined take");
        assert!(!k.metronome_on, "and it does not start clicking on its own");
    }
}

#[cfg(test)]
mod typing_tests {
    use super::*;

    /// The two directions have to agree, or a field would show one number and
    /// accept a different one for it.
    #[test]
    fn what_a_fader_shows_is_what_it_will_read_back() {
        for pos in 0..=100 {
            let gain = fader_to_gain(pos as f32 / 100.0);
            if gain <= 0.0 {
                continue;
            }
            let shown = gain_text(gain);
            // The user retypes what they can see, which includes the unit and
            // the sign — both have to survive the round trip.
            let back = parse_gain(shown.trim_end_matches(" dB")).expect("that was a level");
            let db_in = 20.0 * gain.log10();
            let db_out = 20.0 * back.log10();
            assert!(
                (db_in - db_out).abs() < 0.06,
                "{shown} read back as {db_out:+.3} dB, not {db_in:+.3}"
            );
        }
    }

    #[test]
    fn a_level_at_or_below_the_bottom_of_the_fader_is_off() {
        // The only way to type OFF, and the one people reach for: "-60" and
        // "as quiet as it goes" are the same intention.
        assert_eq!(parse_gain("-60"), Some(0.0));
        assert_eq!(parse_gain("-99"), Some(0.0));
        assert_eq!(parse_gain("-inf"), Some(0.0));
    }

    #[test]
    fn a_level_above_the_top_is_pinned_rather_than_refused() {
        let g = parse_gain("40").expect("a number is a number");
        let db = 20.0 * g.log10();
        assert!(
            (db - f64::from(GAIN_MAX_DB) as f32).abs() < 1e-3,
            "{db} should have been pinned to {GAIN_MAX_DB}"
        );
    }

    #[test]
    fn text_that_is_not_a_number_changes_nothing() {
        // `None` and not a default: committing junk must leave the value the
        // user was already looking at exactly where it was.
        for junk in ["", "   ", "loud", "-", ".", "12x"] {
            assert_eq!(parse_gain(junk), None, "{junk:?} is not a level");
            assert_eq!(parse_bpm(junk), None, "{junk:?} is not a tempo");
        }
    }

    #[test]
    fn a_tempo_is_clamped_to_what_the_smf_writer_can_say() {
        assert_eq!(parse_bpm("120"), Some(120.0));
        assert_eq!(parse_bpm("0"), Some(MIN_BPM));
        assert_eq!(parse_bpm("100000"), Some(MAX_BPM));
        assert_eq!(parse_bpm("-4"), Some(MIN_BPM));
        // NaN and the infinities parse as an f64 and would sail through a
        // range check, because every comparison against NaN is false — and
        // `clamp` PANICS when handed one. Refused outright rather than
        // clamped: neither is reachable from the keyboard (the field drops
        // letters) and neither is a tempo.
        assert_eq!(parse_bpm("NaN"), None);
        assert_eq!(parse_bpm("inf"), None);
        assert_eq!(parse_bpm("-inf"), None);
    }

    /// A field with keyboard focus is a field the app's single-key shortcuts
    /// are typing into. Anything that is not part of a number has to bounce.
    #[test]
    fn the_field_refuses_everything_that_is_not_part_of_a_number() {
        let mut e = NumEdit::new(NumField::Tempo);
        for ch in "r1e2c3".chars() {
            e.push(ch);
        }
        assert_eq!(e.text, "123", "letters reached the field");
    }

    #[test]
    fn one_point_and_a_minus_only_in_front() {
        let mut e = NumEdit::new(NumField::Input);
        for ch in "-1.2.3-".chars() {
            e.push(ch);
        }
        assert_eq!(e.text, "-1.23");
        assert!(parse_gain(&e.text).is_some(), "and it parses");
    }

    #[test]
    fn a_field_starts_empty_so_the_first_digit_replaces() {
        // Seeded with the current reading, "+0.0 dB", somebody wanting -6 would
        // have to delete seven characters before typing anything.
        assert_eq!(NumEdit::new(NumField::Slot(0)).text, "");
    }

    #[test]
    fn a_field_cannot_be_typed_into_forever() {
        let mut e = NumEdit::new(NumField::Tempo);
        for _ in 0..200 {
            e.push('9');
        }
        assert!(e.text.len() <= 8, "the box grew to {}", e.text.len());
        // And what is in it is still a tempo rather than an overflow.
        assert_eq!(parse_bpm(&e.text), Some(MAX_BPM));
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use egui::{Pos2, Rect};

    /// The default panel selection — piano and chord name — as a width:height
    /// ratio. Measured from `band_sizes_at`: at 1300 wide the piano is 150 and
    /// the chord strip is 50, so the content is 1300:200.
    const PIANO_AND_CHORD: f32 = 1300.0 / 200.0;
    /// What almost every camera on a desk is, and what these tests assume
    /// unless they are about a camera that is not.
    const CAM_16_9: f32 = 16.0 / 9.0;

    fn landscape() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(1920.0, 1080.0))
    }
    fn portrait() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(1080.0, 1920.0))
    }

    /// Every layer that IS in the composite has somewhere to go, and every
    /// layer that is not has nowhere. A pane for a layer nobody asked for is a
    /// black rectangle in the finished video.
    #[test]
    fn a_layer_gets_a_pane_exactly_when_it_is_in_the_composite() {
        for l in Layout::ALL {
            for frame in [landscape(), portrait()] {
                for (cam, disp) in [(true, true), (true, false), (false, true), (false, false)] {
                    let p = l.split(frame, cam.then_some(CAM_16_9), disp.then_some(PIANO_AND_CHORD));
                    assert_eq!(p.camera.is_some(), cam, "{l:?} camera");
                    assert_eq!(p.display.is_some(), disp, "{l:?} display");
                }
            }
        }
    }

    /// The only layer in the video gets the WHOLE video. Anything else is a
    /// letterbox around the one thing the user asked for.
    #[test]
    fn one_layer_alone_fills_the_frame() {
        for l in Layout::ALL {
            let f = landscape();
            assert_eq!(l.split(f, Some(CAM_16_9), None).camera, Some(f), "{l:?}");
            assert_eq!(l.split(f, None, Some(PIANO_AND_CHORD)).display, Some(f), "{l:?}");
        }
    }

    /// Stacked panes must tile the frame: no gap, no overlap, nothing outside.
    /// A gap is a black stripe and an overlap is a layer eating another.
    #[test]
    fn stacked_panes_tile_the_frame_exactly() {
        for l in [Layout::CameraAbove, Layout::DisplayAbove, Layout::SideBySide] {
            for frame in [landscape(), portrait()] {
                let p = l.split(frame, Some(CAM_16_9), Some(PIANO_AND_CHORD));
                let (c, d) = (p.camera.expect("camera"), p.display.expect("display"));
                assert!(frame.contains_rect(c) && frame.contains_rect(d), "{l:?} escaped");
                let covered = c.area() + d.area();
                assert!(
                    (covered - frame.area()).abs() < 1.0,
                    "{l:?} covers {covered} of {}",
                    frame.area()
                );
                assert!(!l.overlays(), "{l:?} should not be an overlay");
            }
        }
    }

    /// **The display band fits its content, and the camera gets everything
    /// else.**
    ///
    /// The first version handed the band a flat fraction — 30% landscape, 40%
    /// portrait — and the very first composited frame showed the cost: in a
    /// 360x640 vertical frame the keyboard is 55 points tall and it was given
    /// 256, so five sixths of the band was black and the camera had lost a
    /// quarter of the picture to hold it.
    #[test]
    fn the_display_band_is_the_height_of_what_goes_in_it() {
        for frame in [landscape(), portrait()] {
            let d = Layout::CameraAbove
                .split(frame, Some(CAM_16_9), Some(PIANO_AND_CHORD))
                .display
                .expect("display");
            let natural = frame.width() / PIANO_AND_CHORD;
            assert!(
                (d.height() - natural).abs() < 1.0,
                "the band is {} tall for content that is {natural}",
                d.height()
            );
        }
    }

    /// And it is still CAPPED, because the band grows with every panel switched
    /// on. A fretboard and three theory diagrams must not take the whole frame.
    #[test]
    fn a_very_tall_display_is_capped_rather_than_swallowing_the_frame() {
        // Aspect 1.0 is a square display, far taller than any real selection.
        let f = landscape();
        let d = Layout::CameraAbove
            .split(f, Some(CAM_16_9), Some(1.0))
            .display
            .expect("display");
        assert!(
            d.height() <= f.height() * 0.41,
            "an outsized display took {} of {}",
            d.height(),
            f.height()
        );
        assert!(
            f.contains_rect(d),
            "and it must still be inside the frame"
        );
    }

    /// A vertical frame gives the camera nearly all of itself, which is the
    /// whole reason 9:16 is worth offering: a near-square camera pane is a far
    /// better crop of a person at a piano than 16:9 ever gives.
    #[test]
    fn the_vertical_camera_gets_the_bulk_of_the_frame() {
        let f = portrait();
        let p = Layout::CameraAbove.split(f, Some(CAM_16_9), Some(PIANO_AND_CHORD));
        let cam = p.camera.expect("camera");
        let share = cam.height() / f.height();
        assert!(
            share > 0.80,
            "the camera got {share} of a vertical frame - the keyboard does not \
             need the rest"
        );
    }

    /// In 9:16 the camera pane must stay a usable shape for a person. This is
    /// the assertion that justifies offering vertical at all.
    #[test]
    fn the_vertical_camera_pane_is_not_a_letterbox_slot() {
        let cam = Layout::CameraAbove
            .split(portrait(), Some(CAM_16_9), Some(PIANO_AND_CHORD))
            .camera
            .expect("camera");
        let aspect = cam.width() / cam.height();
        // Portrait-ish, and that is right: a 9:16 frame with a short keyboard
        // strip under it leaves the camera a tall pane, which frames a seated
        // player from head to hands. The bound that matters is that it is not a
        // LETTERBOX SLOT — nothing like the 9:32 sliver a side-by-side split
        // would give it.
        assert!(
            (0.45..=1.6).contains(&aspect),
            "a {aspect:.2}:1 camera pane is not a crop anybody wants of a person"
        );
    }

    /// Side by side in portrait would be two 9:32 slivers, so it stacks.
    /// Silently giving the arrangement that works beats giving the one that was
    /// literally asked for and letting the user find out at the export.
    #[test]
    fn side_by_side_stacks_when_the_frame_is_vertical() {
        let p = Layout::SideBySide.split(portrait(), Some(CAM_16_9), Some(PIANO_AND_CHORD));
        let stacked = Layout::CameraAbove.split(portrait(), Some(CAM_16_9), Some(PIANO_AND_CHORD));
        assert_eq!(p, stacked, "side by side should have stacked in 9:16");
        // And in landscape it is genuinely side by side.
        let wide = Layout::SideBySide.split(landscape(), Some(CAM_16_9), Some(PIANO_AND_CHORD));
        let (c, d) = (wide.camera.unwrap(), wide.display.unwrap());
        assert!((c.height() - landscape().height()).abs() < 1.0);
        assert!(c.right() <= d.left() + 1.0, "they did not sit side by side");
    }

    /// The overlay keeps the camera whole and floats the display over it, which
    /// is the one case where the panes are SUPPOSED to intersect.
    #[test]
    fn the_overlay_keeps_the_camera_whole_and_sits_on_top_of_it() {
        let f = landscape();
        let p = Layout::CameraFull.split(f, Some(CAM_16_9), Some(PIANO_AND_CHORD));
        assert_eq!(p.camera, Some(f), "the camera should still fill the frame");
        let d = p.display.expect("display");
        assert!(f.contains_rect(d));
        assert!(d.intersects(f));
        assert!(Layout::CameraFull.overlays(), "the compositor must paint it last");
        assert!(
            d.height() < f.height() * 0.30,
            "an overlay covering a third of the picture is not an overlay"
        );
    }

    /// **The default puts the APP on screen and the camera in the corner.**
    ///
    /// It was the other way round, and that made the thing worth watching the
    /// secondary one: a fretboard and three theory diagrams squeezed into a
    /// band under a webcam are too small to read, which is the whole reason
    /// anybody would record them.
    #[test]
    fn the_default_layout_gives_the_display_the_frame() {
        assert_eq!(Layout::default(), Layout::DisplayFull);
        for frame in [landscape(), portrait()] {
            let p = Layout::DisplayFull.split(frame, Some(CAM_16_9), Some(PIANO_AND_CHORD));
            assert_eq!(
                p.display,
                Some(frame),
                "the app should have the whole frame"
            );
            let cam = p.camera.expect("camera");
            assert!(frame.contains_rect(cam), "the inset escaped the frame");
            // **The SENSOR's shape**, whatever that is, because the compositor
            // centre-crops the camera into whatever pane it is handed — so a
            // pane of the wrong shape silently throws away the sides or the top
            // and bottom of every frame the camera delivers.
            for sensor in [CAM_16_9, 4.0 / 3.0, 1.0] {
                let c = Layout::DisplayFull
                    .split(frame, Some(sensor), Some(PIANO_AND_CHORD))
                    .camera
                    .expect("camera");
                assert!(
                    (c.width() / c.height() - sensor).abs() < 0.01,
                    "a {sensor}:1 sensor got a {}:1 pane",
                    c.width() / c.height()
                );
            }
            // Small enough to be context, big enough to be a person — and the
            // SAME apparent size in both frames, which is the whole reason the
            // height comes off the short edge. A fraction of the width would be
            // a quarter of a 16:9 frame and a fifteenth of a 9:16 one.
            let short = frame.width().min(frame.height());
            let share = cam.height() / short;
            assert!(
                (0.14..=0.22).contains(&share),
                "the inset is {share} of the short edge tall"
            );
            // It cannot walk across the picture however wide the camera is.
            let widest = Layout::DisplayFull
                .split(frame, Some(40.0), Some(PIANO_AND_CHORD))
                .camera
                .expect("camera");
            assert!(
                widest.width() <= frame.width() * 0.34,
                "a very wide camera took {} of the frame",
                widest.width() / frame.width()
            );
            // Clear of the edges, and in the TOP right: the keyboard and the
            // fretboard run full width along the bottom, and the top right is
            // the end of the theory band, which is where the air is.
            assert!(cam.right() < frame.right() && cam.top() > frame.top());
            assert!(cam.left() > frame.center().x && cam.bottom() < frame.center().y);
        }
    }

    /// The compositor has to know WHICH layer floats. `CameraFull` puts the
    /// display over the camera; `DisplayFull` puts the camera over the display.
    /// Painting the camera first regardless buried the inset under the app,
    /// which is the same as not drawing it.
    #[test]
    fn the_overlaid_layer_is_named_and_the_two_full_frames_disagree() {
        assert!(Layout::CameraFull.overlays());
        assert!(!Layout::CameraFull.camera_on_top());
        assert!(Layout::DisplayFull.overlays());
        assert!(Layout::DisplayFull.camera_on_top());
        for l in [Layout::CameraAbove, Layout::DisplayAbove, Layout::SideBySide] {
            assert!(!l.overlays(), "{l:?} is stacked, not overlaid");
            assert!(!l.camera_on_top(), "{l:?} floats nothing");
        }
    }

    /// A degenerate frame must not produce a pane with negative size, which is
    /// what a compositor would then try to render into.
    #[test]
    fn a_frame_with_no_area_produces_no_impossible_panes() {
        for w in [0.0_f32, 1.0] {
            for h in [0.0_f32, 1.0] {
                let f = Rect::from_min_size(Pos2::ZERO, egui::vec2(w, h));
                for l in Layout::ALL {
                    let p = l.split(f, Some(CAM_16_9), Some(PIANO_AND_CHORD));
                    for r in [p.camera, p.display].into_iter().flatten() {
                        assert!(
                            r.width() >= 0.0 && r.height() >= 0.0,
                            "{l:?} produced {r:?} from {f:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_vertical_resolution_is_a_real_portrait_frame() {
        assert_eq!(Resolution::Vertical1080.pixels(), Some((1080, 1920)));
        assert!(Resolution::Vertical1080.is_portrait(None));
        assert!(!Resolution::Hd1080.is_portrait(None));
        // And a phone held upright, with MatchCamera, is portrait too.
        assert!(Resolution::MatchCamera.is_portrait(Some((1080, 1920))));
        assert!(!Resolution::MatchCamera.is_portrait(Some((1920, 1080))));
    }

    /// Every resolution round-trips through its settings key, or a saved
    /// vertical export silently becomes 1080p the next time the app starts.
    #[test]
    fn every_resolution_survives_the_settings_file() {
        for r in Resolution::ALL {
            assert_eq!(Resolution::from_key(r.key()), Some(r), "{r:?}");
        }
        for l in Layout::ALL {
            assert_eq!(Layout::from_key(l.key()), Some(l), "{l:?}");
        }
    }
}

#[cfg(test)]
mod meter_tests {
    use super::*;

    /// **The tempo counts quarters, whatever the unit is.**
    ///
    /// Forced by the file format: an SMF tempo meta event is microseconds per
    /// quarter note and nothing else, so any other reading makes the `.mid`
    /// disagree with the click in every bar.
    #[test]
    fn a_beat_is_measured_against_the_quarter_note() {
        let bpm = 120.0;
        // A quarter at 120 is half a second, and 4/4 is four of them.
        let four_four = TimeSignature { beats: 4, unit: 4 };
        assert!((four_four.beat_seconds(bpm) - 0.5).abs() < 1e-12);

        // An eighth is half of that, so a bar of 6/8 is a second and a half.
        let six_eight = TimeSignature { beats: 6, unit: 8 };
        assert!((six_eight.beat_seconds(bpm) - 0.25).abs() < 1e-12);
        let bar = six_eight.beat_seconds(bpm) * f64::from(six_eight.beats);
        assert!((bar - 1.5).abs() < 1e-12, "a 6/8 bar at 120 is 1.5s, got {bar}");

        // And a half-note gets two quarters.
        let cut = TimeSignature { beats: 2, unit: 2 };
        assert!((cut.beat_seconds(bpm) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_signature_is_two_numbers_and_a_note_value() {
        assert!(TimeSignature { beats: 4, unit: 4 }.is_valid());
        assert!(TimeSignature { beats: 7, unit: 8 }.is_valid());
        assert!(TimeSignature { beats: 1, unit: 32 }.is_valid());
        // The unit has to be a power of two, because that is what a note value
        // IS — and because the SMF meta stores it as a power.
        assert!(!TimeSignature { beats: 4, unit: 6 }.is_valid());
        assert!(!TimeSignature { beats: 4, unit: 0 }.is_valid());
        assert!(!TimeSignature { beats: 0, unit: 4 }.is_valid());
        assert!(!TimeSignature { beats: 33, unit: 4 }.is_valid());
    }

    /// The SMF writes the unit as a POWER of two: 4 is 2, 8 is 3. Getting it
    /// wrong writes a file every DAW reads as a different signature, and a bar
    /// line in the wrong place is not something anybody notices until later.
    #[test]
    fn the_smf_meta_stores_the_unit_as_a_power_of_two() {
        for (unit, power) in [(1, 0), (2, 1), (4, 2), (8, 3), (16, 4), (32, 5)] {
            let sig = TimeSignature { beats: 4, unit };
            assert_eq!(sig.unit_power(), power, "{unit} should write as {power}");
        }
    }

    /// Every offered signature is playable, and round-trips through the text
    /// the settings file and the menu both use.
    #[test]
    fn every_offered_signature_survives_the_settings_file() {
        for sig in TIME_SIGNATURES {
            assert!(sig.is_valid(), "{} is offered and unplayable", sig.label());
            assert_eq!(TimeSignature::parse(&sig.label()), Some(sig));
        }
        // And a custom one the menu never offered.
        assert_eq!(
            TimeSignature::parse("11/16"),
            Some(TimeSignature { beats: 11, unit: 16 })
        );
        assert_eq!(TimeSignature::parse(" 3 / 4 "), Some(TimeSignature { beats: 3, unit: 4 }));
        // Nonsense is None rather than a guess.
        for bad in ["", "4", "4/", "/4", "4/6", "0/4", "x/y", "4/4/4"] {
            assert_eq!(TimeSignature::parse(bad), None, "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_count_in_is_bars_times_the_top_number() {
        assert_eq!(TimeSignature { beats: 4, unit: 4 }.beats_in(2), 8);
        assert_eq!(TimeSignature { beats: 6, unit: 8 }.beats_in(1), 6);
        assert_eq!(TimeSignature { beats: 7, unit: 8 }.beats_in(2), 14);
        assert_eq!(TimeSignature { beats: 4, unit: 4 }.beats_in(0), 0);
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    fn side(rate: u32, frames: Option<u32>) -> (String, StreamStats) {
        (
            "device".to_owned(),
            StreamStats {
                sample_rate: rate,
                channels: 2,
                buffer_frames: frames,
            },
        )
    }

    #[test]
    fn a_buffer_is_reported_in_milliseconds_of_its_own_rate() {
        // 256 frames at 48k is 5.333 ms; at 44.1k it is longer, which is the
        // whole reason the rate is part of the sum.
        let at48 = StreamStats {
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: Some(256),
        };
        assert!((at48.buffer_ms().unwrap() - 5.3333).abs() < 1e-3);
        let at44 = StreamStats {
            sample_rate: 44_100,
            channels: 2,
            buffer_frames: Some(256),
        };
        assert!(at44.buffer_ms().unwrap() > at48.buffer_ms().unwrap());

        // A device that chose its own size did not say what it chose, and
        // inventing a number here would be inventing a latency figure.
        assert_eq!(
            StreamStats {
                sample_rate: 48_000,
                channels: 2,
                buffer_frames: None,
            }
            .buffer_ms(),
            None
        );
    }

    /// The round trip is BOTH sides, and absent when either is.
    #[test]
    fn the_round_trip_needs_both_halves() {
        let both = AudioStatus {
            input: Some(side(48_000, Some(256))),
            output: Some(side(48_000, Some(512))),
        };
        assert!((both.round_trip_ms().unwrap() - 16.0).abs() < 1e-3);

        for one in [
            AudioStatus {
                input: Some(side(48_000, Some(256))),
                output: None,
            },
            AudioStatus {
                input: None,
                output: Some(side(48_000, Some(256))),
            },
            // And a side that will not say its buffer size cannot contribute
            // half a figure.
            AudioStatus {
                input: Some(side(48_000, None)),
                output: Some(side(48_000, Some(256))),
            },
        ] {
            assert_eq!(one.round_trip_ms(), None);
        }
    }

    /// **A rate mismatch is a fault, not a curiosity.**
    ///
    /// The writer drains the instrument's ring at the INPUT's rate while the
    /// engine fills it at the OUTPUT's, so a mismatch overflows that ring and
    /// reports the losses against the take — a symptom that points nowhere near
    /// its cause, which is exactly why the panel says it out loud.
    #[test]
    fn a_rate_mismatch_is_noticed() {
        assert!(AudioStatus {
            input: Some(side(44_100, Some(256))),
            output: Some(side(48_000, Some(256))),
        }
        .rates_disagree());

        assert!(!AudioStatus {
            input: Some(side(48_000, Some(256))),
            output: Some(side(48_000, Some(512))),
        }
        .rates_disagree());

        // One side alone cannot disagree with anything.
        assert!(!AudioStatus {
            input: Some(side(44_100, Some(256))),
            output: None,
        }
        .rates_disagree());
    }
}

#[cfg(test)]
mod missing_tests {
    use super::{missing_from_take, Strip, SLOTS};

    /// **The one that costs a take, so the sentence has to be an
    /// instruction.**
    ///
    /// Mute is now the only thing that decides what a file is made of. That is
    /// one rule instead of four, and worth it — but a rule nobody is told
    /// about is exactly the setting it replaced, which cost the owner twelve
    /// takes across three releases.
    #[test]
    fn it_names_what_is_lost_and_where_to_fix_it() {
        assert_eq!(missing_from_take(&[]), None, "nothing muted, nothing to say");

        let one = missing_from_take(&[Strip::Input(0)]).expect("a muted input is news");
        assert!(
            one.contains("input 1 is muted") && one.contains("will not have it"),
            "{one}"
        );
        assert!(
            one.contains("Tab"),
            "it has to say where the mixer is, or it is a complaint rather than \
             an instruction: {one}"
        );

        // A list anybody would say out loud, and the verb agreeing with it.
        // "instrument 2 are muted" is the kind of thing that makes a person
        // trust the rest of the line less.
        let many = missing_from_take(&[Strip::Slot(1), Strip::Input(0), Strip::Track])
            .expect("three of them is still news");
        assert!(
            many.contains("instrument 2, input 1 and the backing track"),
            "{many}"
        );
        assert!(many.contains("are muted") && many.contains("have them"), "{many}");
    }

    /// A slot says WHICH slot. "the instrument is muted" on a rack of five is
    /// not something anybody can act on.
    #[test]
    fn a_slot_says_which_one() {
        for i in 0..SLOTS {
            let said = Strip::Slot(i).label();
            assert_eq!(said, format!("instrument {}", i + 1));
        }
    }
}
