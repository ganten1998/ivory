//! Capture devices, as the shared GUI is allowed to see them.
//!
//! `ivory-ui` may not link `cpal` or anything that opens a camera — that is the
//! firewall `scripts/check-firewall.sh` asserts — so it talks to
//! [`ivory_ui::ports::CaptureDevices`] and this is what stands behind it.
//!
//! # These impls do not open anything, and that is deliberate
//!
//! `open()` records an intention; it does not touch hardware. Opening happens
//! in [`crate::desktop`], after the frame, when the reconciler notices that
//! what the user wants and what the [`crate::record::Session`] has open have
//! diverged.
//!
//! Two reasons, and the second is the load-bearing one:
//!
//! * Opening a device from inside an egui frame means doing a slow, blocking,
//!   possibly permission-prompting thing in the middle of painting.
//! * `cpal::Stream` is `!Send`, so the session that owns it cannot be reached
//!   through the `Send` trait object the app holds. Anything else here would be
//!   a lie the compiler would eventually catch, or an `Arc<Mutex<>>` around
//!   something that must never leave the UI thread.
//!
//! So what crosses the boundary is a `String` of intent, under a mutex, which
//! is `Send` because it is data.

use ivory_record::audio::ChannelPick;
use ivory_ui::ports::{CaptureDevices, DeviceInfo};
use std::sync::{Arc, Mutex};

/// What the picker chose and what the reconciler managed to do about it.
///
/// One struct for both directions because they are two halves of one
/// conversation, and splitting them into two mutexes would let the name and the
/// uid be read from different moments.
#[derive(Debug, Default)]
pub struct Selection {
    /// The uid the user picked. `None` means no device.
    ///
    /// **Read it together with [`explicit`](Self::explicit).** `None` alone is
    /// ambiguous and the ambiguity was a real bug: "the user has never opened
    /// the picker, give them the system default so the meter is live" and "the
    /// user chose *None - record MIDI only*" are opposite instructions, and
    /// mapping both to `None` meant picking None opened the built-in
    /// microphone and showed its name in the band.
    pub wanted: Option<String>,
    /// The OTHER inputs open at the same time, as channel uids.
    ///
    /// **All of the same device as `wanted`.** One interface is one clock, and
    /// the app declines a second device on purpose — anyone with that rig
    /// makes an aggregate device, which presents as one. Anything here whose
    /// device key does not match `wanted` is dropped rather than obeyed, so a
    /// settings file from a machine with different hardware costs the extra
    /// inputs and not the microphone.
    ///
    /// `wanted` is input 1 and these are inputs 2 upward, in order.
    pub extra: Vec<String>,
    /// The user has actually made a choice, as opposed to never having looked.
    pub explicit: bool,
    /// The uid actually open. Lags `wanted` by up to one frame.
    pub open: Option<String>,
    /// Display name of whatever is open, for the band.
    pub open_name: Option<String>,
    /// Why the last open failed, in words meant for the user.
    pub error: Option<String>,
    /// A reconcile has run at least once for this selection.
    ///
    /// Needed because `wanted == open == None` is both the initial state and
    /// the settled result of choosing None, and only the second one means "the
    /// camera has already been closed, do not close it again".
    pub settled: bool,
    /// Inputs the picker offers as rows of their own, as channel uids.
    ///
    /// **A set, not a choice, and that is the whole design.** An interface's
    /// inputs are not alternatives: the piano is on 1/2, a room mic is on 6,
    /// and which of them a take wants changes between takes. So the chooser
    /// says which ones EXIST as far as the picker is concerned, one row each,
    /// and the picker still says which one is open — the same relationship the
    /// device list already has with the device that is running.
    ///
    /// Spans every device, not just the one selected: unplugging an interface
    /// must not forget how it was set up, and `list` only ever emits a row for
    /// a device it can actually see.
    pub exposed: Vec<String>,
}

impl Selection {
    /// Whether the reconciler has work to do.
    pub fn is_stale(&self) -> bool {
        !self.settled || self.wanted != self.open
    }
}

/// A shared selection, handed to both the trait object and the reconciler.
pub type Shared = Arc<Mutex<Selection>>;

/// Recover from a poisoned lock rather than propagating it.
///
/// The only thing under these mutexes is four `Option<String>`s. A panic while
/// holding one cannot leave them in an inconsistent state, and refusing to let
/// the user pick a microphone because an unrelated thread panicked would be a
/// worse outcome than any invariant this could be protecting.
fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Selection> {
    match shared.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Audio inputs
// ───────────────────────────────────────────────────────────────────────────

/// Every audio input `cpal` can see, plus the selection.
pub struct AudioInputs(Shared);

impl AudioInputs {
    pub fn new() -> (Self, Shared) {
        let shared: Shared = Arc::new(Mutex::new(Selection::default()));
        (Self(Arc::clone(&shared)), shared)
    }
}

/// The separator between a device's key and one of its inputs, inside a uid.
///
/// **ASCII 31, UNIT SEPARATOR, and the choice is the point.** The uid is
/// documented as opaque and is never shown, so its grammar is this module's to
/// define — but it is also stored in `settings.json` and round-tripped, so it
/// has to survive a device whose own name contains anything a device name can
/// contain. `#` was already taken and escaped by `DeviceKey`; `|`, `:` and `@`
/// all appear in real interface names. A C0 control character appears in none,
/// serialises as `\u001f`, and is invisible in the file.
const CHANNEL_SEP: char = '\u{1f}';

/// Marks a uid as naming a PipeWire node rather than a cpal device.
///
/// Node names are `alsa_input.…`, `bluez_input.…`, `v4l2_input.…` — never this
/// — and the uid is opaque and never shown, so a prefix is enough to tell the
/// two grammars apart in a settings file that has to hold both.
const PIPEWIRE_UID: &str = "pw:";

/// Split a uid into the device's key and the input picked out of it, if any.
///
/// A malformed channel — anything that is not a number — is treated as no
/// channel rather than as an error, and the DEVICE still opens. A settings file
/// somebody edited by hand should cost the channel, not the microphone.
fn split_channel(uid: &str) -> (&str, Option<ChannelPick>) {
    let Some((key, spec)) = uid.split_once(CHANNEL_SEP) else {
        return (uid, None);
    };
    (key, parse_pick(spec))
}

/// `3` is mono input 3; `1+2` is a stereo pair of inputs 1 and 2.
///
/// Zero-based, like everything below the label. `+` because it cannot appear
/// in a number and reads as "and" — and because a `-` would be ambiguous with
/// a range, which this is deliberately not: 3+0 is a legitimate pair with the
/// channels crossed, and a range cannot say that.
fn parse_pick(spec: &str) -> Option<ChannelPick> {
    match spec.split_once('+') {
        Some((a, b)) => Some(ChannelPick::Stereo(a.parse().ok()?, b.parse().ok()?)),
        None => Some(ChannelPick::Mono(spec.parse().ok()?)),
    }
}

/// The uid for one input of a device, or a pair of them.
fn channel_uid(key: &str, pick: ChannelPick) -> String {
    match pick {
        ChannelPick::Mono(a) => format!("{key}{CHANNEL_SEP}{a}"),
        ChannelPick::Stereo(a, b) => format!("{key}{CHANNEL_SEP}{a}+{b}"),
    }
}

/// A device's name with the input it is showing, for a picker row and for the
/// band's own label.
///
/// One function so the two can never word it differently: choosing
/// "Scarlett  -  inputs 1/2" in the picker and reading something else in the
/// band is how somebody ends up unsure which input a take actually holds.
fn with_channel(name: &str, pick: ChannelPick) -> String {
    let word = if pick.channels() == 2 { "inputs" } else { "input" };
    format!("{name}  -  {word} {}", channel_label(pick))
}

/// Which input of a device a pick names, as bare numbers: "3", "4/5".
///
/// **Separable from the device's name on purpose.** A mixer strip has room for
/// one of the two, and the half worth keeping is this one — the interface is
/// the same on every channel of it, so the desk puts a short name in front and
/// this after: "x - 3", "x - 4/5".
///
/// One-based, because that is what is printed on the box.
pub fn channel_label(pick: ChannelPick) -> String {
    match pick {
        ChannelPick::Mono(a) => format!("{}", a + 1),
        ChannelPick::Stereo(a, b) => format!("{}/{}", a + 1, b + 1),
    }
}

/// Where a pick sorts among its device's rows: by first input, then by width.
fn pick_order(pick: ChannelPick) -> (u16, u16) {
    match pick {
        ChannelPick::Mono(a) => (a, a),
        ChannelPick::Stereo(a, b) => (a, b),
    }
}

/// Follow each device with the inputs of it the user asked to see.
///
/// **Rows, not a mode.** The alternative was what this replaced: one hidden
/// "which input" setting attached to the selected device, so recording the
/// room mic meant opening a panel and changing it back afterwards. An
/// interface's inputs are separate sources; they belong in the list of sources
/// beside the interface itself, and picking one is picking a microphone.
fn with_exposed(found: Vec<DeviceInfo>, exposed: &[String]) -> Vec<DeviceInfo> {
    let mut out: Vec<DeviceInfo> = Vec::with_capacity(found.len());
    for d in found {
        let mut extras: Vec<(ChannelPick, String)> = exposed
            .iter()
            .filter_map(|uid| {
                let (key, pick) = split_channel(uid);
                (key == d.uid).then_some(()).and(pick).map(|p| (p, uid.clone()))
            })
            .collect();
        extras.sort_by_key(|(p, _)| pick_order(*p));
        extras.dedup_by(|a, b| a.1 == b.1);
        let name = d.name.clone();
        out.push(d);
        for (pick, uid) in extras {
            out.push(DeviceInfo {
                uid,
                name: with_channel(&name, pick),
                // The system default is the DEVICE. A row for one of its
                // inputs is this app's invention and the OS has no opinion
                // about it, so it never wears that badge.
                default: false,
            });
        }
    }
    out
}

impl CaptureDevices for AudioInputs {
    fn list(&self) -> Vec<DeviceInfo> {
        // Not cached, deliberately: an interface plugged in while the Recorder
        // band is open must appear without a restart, and cpal has no change
        // notification to hang a cache invalidation on.
        let Ok(found) = ivory_record::audio::input_devices() else {
            // An empty world on macOS is what a missing microphone entitlement
            // looks like (RECORDER-PLAN §0). The band says "no audio inputs"
            // either way, which is the honest report — it genuinely cannot see
            // any, and guessing at the reason here would be guessing.
            return Vec::new();
        };
        let mut out = Vec::new();
        // **PipeWire first, and on its own when it answers.**
        //
        // On a PipeWire machine, cpal's list is a lie of omission: PipeWire
        // holds every card exclusively, ALSA returns `EBUSY` for each one, and
        // the single PCM that opens is `pipewire` — which follows whatever the
        // desktop's default source happens to be. The owner had a Scarlett
        // plugged in, one entry in the picker called `pipewire`, and a
        // recording of the laptop's built-in microphone.
        //
        // So when PipeWire answers, its answer REPLACES the cpal list rather
        // than joining it: the cpal entries are the same hardware reached
        // through a route that cannot be aimed, and offering both would be
        // offering the broken one beside the working one with nothing to tell
        // them apart. An empty answer — no PipeWire, no `pw-dump` — falls
        // through to exactly what every earlier build did.
        for src in ivory_record::audio::pipewire_sources() {
            let uid = format!("{PIPEWIRE_UID}{}", src.node);
            out.push(DeviceInfo {
                uid: uid.clone(),
                name: src.description.clone(),
                // PipeWire has a default source, but `pw-dump` does not mark
                // it in the node's own props. Left unmarked rather than
                // guessed: a badge on the wrong row is worse than no badge.
                default: false,
            });
        }
        if !out.is_empty() {
            return with_exposed(out, &lock(&self.0).exposed);
        }
        for d in found {
            // The uid is the name-plus-occurrence key, not the bare name:
            // `DeviceKey` exists because two identical interfaces report the
            // same string and cpal offers no way to compare devices.
            let key = d.key.to_setting();
            out.push(DeviceInfo {
                uid: key.clone(),
                name: d.key.to_string(),
                default: d.is_default,
            });
        }
        with_exposed(out, &lock(&self.0).exposed)
    }

    fn open(&mut self, uid: &str) -> Result<(), String> {
        let mut sel = lock(&self.0);
        sel.wanted = (!uid.is_empty()).then(|| uid.to_owned());
        // Choosing the None row IS a choice, and the whole point of this flag.
        sel.explicit = true;
        sel.error = None;
        Ok(())
    }

    fn current(&self) -> Option<String> {
        lock(&self.0).wanted.clone()
    }

    fn current_name(&self) -> Option<String> {
        let sel = lock(&self.0);
        let name = sel.open_name.clone()?;
        // The CHANNEL too, when one was chosen, or the band would say
        // "Scarlett 18i20" whether it is recording the whole interface or the
        // one input the piano is in — which are different takes.
        //
        // Read off the selection rather than off the open stream: the stream
        // knows only that it is mono. The two can disagree in one case, a
        // device that came back from a hub with fewer inputs than the saved
        // channel number, where the stream falls back to everything and this
        // still names the input that was asked for. Re-picking settles it.
        match sel.wanted.as_deref().and_then(|u| split_channel(u).1) {
            Some(pick) => Some(with_channel(&name, pick)),
            None => Some(name),
        }
    }
}

/// A device and, when the user picked one, the input of it to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioChoice {
    pub selection: ivory_record::audio::InputSelection,
    /// Which inputs. Zero-based, and never yet known to exist on the device
    /// that opens — that is checked where the stream is built.
    pub channels: ivory_record::audio::Picks,
}

/// The selection as `ivory-record` wants it, or `None` for "open nothing".
///
/// Never-chosen becomes the host's own default, so somebody who has never
/// opened the picker still gets a live meter — that is the entire point of the
/// meter being live before arming. Explicitly-chosen-None becomes `None`, and
/// the caller closes the device.
pub fn audio_selection(shared: &Shared) -> Option<AudioChoice> {
    use ivory_record::audio::{DeviceKey, InputSelection};
    let sel = lock(shared);
    match sel.wanted.as_deref() {
        Some(uid) => {
            let (key, channel) = split_channel(uid);
            let selection = match key.strip_prefix(PIPEWIRE_UID) {
                Some(node) => InputSelection::PipewireNode(node.to_owned()),
                None => InputSelection::Key(DeviceKey::from_setting(key)),
            };
            // **The extras, and only the ones on the same box.** A uid from
            // another device is not a second interface this app can open; it
            // is a leftover, and obeying it would open a channel number
            // against hardware that never had it.
            let mut picks: Vec<ChannelPick> = channel.into_iter().collect();
            if !picks.is_empty() {
                for uid in &sel.extra {
                    let (k, pick) = split_channel(uid);
                    if k != key {
                        continue;
                    }
                    if let Some(p) = pick {
                        if !picks.contains(&p) {
                            picks.push(p);
                        }
                    }
                }
            }
            let channels = ivory_record::audio::Picks::from_slice(&picks);
            Some(AudioChoice { selection, channels })
        }
        // **Nothing chosen means nothing opened.** There used to be a fallback
        // to `InputSelection::Default` here, for the case where the user had
        // never opened the picker — the idea being that a live meter is a
        // better first impression than a dead one.
        //
        // It is not worth what it costs. "The system default input" is whatever
        // the OS happens to be pointing at, which on a machine with Loopback or
        // a virtual device installed is a mic that records the wrong thing or
        // nothing at all — and the user's complaint was exactly that: the app
        // kept coming up on a Virtual Mic they had never chosen. It also meant
        // raising the microphone permission prompt at launch, for a device
        // nobody asked for.
        //
        // An app should not open your microphone until you say which one. The
        // band already says "no audio input selected", which is a prompt rather
        // than a fault, and a plugin-only take needs no input at all.
        None => None,
    }
}

/// Write back what actually happened, so the band can show it.
///
/// Always called after an attempt, success or failure. A failed open that left
/// `open != wanted` would be retried every frame forever.
pub fn settle(shared: &Shared, opened: Option<String>, name: Option<String>, error: Option<String>) {
    let mut sel = lock(shared);
    sel.open = opened;
    sel.settled = true;
    sel.open_name = name;
    sel.error = error;
}

/// What the audio system itself can be asked, for the Setup panel.
///
/// A unit struct: both questions are answered by `ivory_record::audio` from
/// process-global state, so there is nothing to hold. It is a trait object all
/// the same because `ivory-ui` may not name cpal — that is what the firewall
/// is — and because a build with no audio hands over `None` instead.
pub struct Setup(Shared);

impl Setup {
    pub fn new(shared: &Shared) -> Self {
        Self(Arc::clone(shared))
    }
}

impl ivory_ui::ports::AudioSetup for Setup {
    fn systems(&self) -> Vec<String> {
        ivory_record::audio::systems()
    }

    fn input_channels(&self) -> u16 {
        let want = lock(&self.0).wanted.clone();
        let Some(uid) = want else {
            return 0;
        };
        let (key, _) = split_channel(&uid);
        // PipeWire knows its own node's width; ALSA's answer comes from the
        // device's default config. Either way this is what the chooser lays
        // its buttons out from, so a wrong number is a chooser that offers
        // inputs the device does not have.
        if let Some(node) = key.strip_prefix(PIPEWIRE_UID) {
            return ivory_record::audio::pipewire_sources()
                .into_iter()
                .find(|s| s.node == node)
                .and_then(|s| s.channels)
                .unwrap_or(0);
        }
        let key = ivory_record::audio::DeviceKey::from_setting(key);
        ivory_record::audio::input_devices()
            .ok()
            .into_iter()
            .flatten()
            .find(|d| d.key == key)
            .and_then(|d| d.channels)
            .unwrap_or(0)
    }

    fn exposed(&self) -> Vec<(u16, Option<u16>)> {
        let sel = lock(&self.0);
        let Some(uid) = sel.wanted.as_deref() else {
            return Vec::new();
        };
        // The device the picker is pointed at, whether or not the row it is
        // pointed at is one of its channels.
        let (key, _) = split_channel(uid);
        let mut picks: Vec<ChannelPick> = sel
            .exposed
            .iter()
            .filter_map(|u| {
                let (k, p) = split_channel(u);
                (k == key).then_some(()).and(p)
            })
            .collect();
        picks.sort_by_key(|p| pick_order(*p));
        picks.dedup();
        picks
            .into_iter()
            .map(|p| match p {
                ChannelPick::Mono(a) => (a, None),
                ChannelPick::Stereo(a, b) => (a, Some(b)),
            })
            .collect()
    }

    fn set_exposed(&mut self, picks: Vec<(u16, Option<u16>)>) -> Vec<String> {
        let mut sel = lock(&self.0);
        let Some(uid) = sel.wanted.clone() else {
            return sel.exposed.clone();
        };
        let (key, open_pick) = split_channel(&uid);
        let key = key.to_owned();
        // Everything belonging to some OTHER device is left exactly as it is.
        // The chooser only ever shows one interface, and a panel that silently
        // forgot the others would lose a setup every time somebody swapped a
        // box over.
        let mut all: Vec<String> = sel
            .exposed
            .iter()
            .filter(|u| split_channel(u).0 != key)
            .cloned()
            .collect();
        let mut mine: Vec<String> = picks
            .into_iter()
            .map(|(a, b)| {
                channel_uid(
                    &key,
                    match b {
                        Some(b) => ChannelPick::Stereo(a, b),
                        None => ChannelPick::Mono(a),
                    },
                )
            })
            .collect();
        mine.sort();
        mine.dedup();
        all.extend(mine.iter().cloned());
        sel.exposed = all.clone();
        // **A row that has just stopped existing must not stay selected.**
        // Unticking the input you are recording leaves the picker pointing at
        // a row that is no longer in the list; falling back to the whole device
        // keeps a microphone open, which is the difference between a tidy-up
        // and a silent take.
        if open_pick.is_some() && !mine.contains(&uid) {
            sel.wanted = Some(key);
            // The stream reopens, exactly as a device change does.
            sel.explicit = true;
            sel.error = None;
        }
        all
    }

    fn rates(&self) -> Vec<u32> {
        // The rates of the device the user has SELECTED, and the host's
        // default only when they have selected nothing. Not the device that is
        // OPEN: a panel opened while an interface is unplugged should offer
        // that interface's rates rather than silently offering the built-in
        // microphone's under its name.
        //
        // The channel is dropped on the way past. Every input of a device runs
        // at the device's rate — that is what a device rate is — so a panel
        // that offered a different list for input 3 would be inventing one.
        match audio_selection(&self.0) {
            Some(choice) => ivory_record::audio::input_rates(&choice.selection),
            None => ivory_record::audio::input_rates(
                &ivory_record::audio::InputSelection::Default,
            ),
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Cameras
// ───────────────────────────────────────────────────────────────────────────

/// Every camera the system can see, plus the selection.
///
/// The same shape as [`AudioInputs`] and for the same reasons, with one real
/// difference: `ivory_record::camera::cameras()` distinguishes "no cameras" from
/// "not allowed to look", and this is where that distinction stops being
/// available. It is preserved by [`Cameras::denied`], which the band reads, so
/// the user is sent to System Settings rather than to a shop.
pub struct Cameras {
    shared: Shared,
    /// Sticky. Enumeration happens whenever a picker opens, but the band draws
    /// every frame and must not enumerate at 60 Hz to find out whether to show
    /// the permission note.
    denied: Arc<Mutex<Option<String>>>,
}

impl Cameras {
    pub fn new() -> (Self, Shared, Arc<Mutex<Option<String>>>) {
        let shared: Shared = Arc::new(Mutex::new(Selection::default()));
        let denied = Arc::new(Mutex::new(None));
        (
            Self {
                shared: Arc::clone(&shared),
                denied: Arc::clone(&denied),
            },
            shared,
            denied,
        )
    }
}

impl CaptureDevices for Cameras {
    fn list(&self) -> Vec<DeviceInfo> {
        match ivory_record::camera::cameras() {
            Ok(found) => {
                if let Ok(mut d) = self.denied.lock() {
                    *d = None;
                }
                found
                    .into_iter()
                    .map(|c| DeviceInfo {
                        // A real UID, unlike the audio side: two identical
                        // webcams have different ones and a UID survives an OS
                        // language change, which `localizedName` does not.
                        uid: c.uid,
                        name: c.name,
                        default: c.is_default,
                    })
                    .collect()
            }
            Err(e) => {
                // Recorded rather than discarded. "No cameras found" and "you
                // said no to the permission prompt and it will never appear
                // again" send the user to completely different places.
                if let Ok(mut d) = self.denied.lock() {
                    *d = Some(e.to_string());
                }
                Vec::new()
            }
        }
    }

    fn open(&mut self, uid: &str) -> Result<(), String> {
        let mut sel = lock(&self.shared);
        sel.wanted = (!uid.is_empty()).then(|| uid.to_owned());
        sel.explicit = true;
        sel.error = None;
        Ok(())
    }

    fn current(&self) -> Option<String> {
        lock(&self.shared).wanted.clone()
    }

    fn current_name(&self) -> Option<String> {
        lock(&self.shared).open_name.clone()
    }
}

/// What each open input is called and how wide it is, in strip order.
///
/// **The primary first**, because `wanted` is input 1 and the extras are
/// inputs 2 upward — the same order `audio_selection` builds the picks in, and
/// the same order the capture lays their channels out in. Three orders that
/// have to agree, and they agree by all coming from here.
///
/// Empty while nothing is open. An extra whose device key does not match the
/// primary's is dropped, exactly as `audio_selection` drops it.
pub fn open_inputs(shared: &Shared) -> Vec<(String, String, bool)> {
    let sel = lock(shared);
    let (Some(name), Some(uid)) = (sel.open_name.clone(), sel.wanted.clone()) else {
        return Vec::new();
    };
    let (key, pick) = split_channel(&uid);
    let described = |p: Option<ChannelPick>| match p {
        Some(p) => (name.clone(), channel_label(p), p.channels() == 2),
        // The whole device: as wide as it is, and a stereo interface is the
        // ordinary case.
        None => (name.clone(), String::new(), true),
    };
    let mut out = vec![described(pick)];
    // Only when the primary is one channel of the device. With the whole
    // device open there is nothing left to add beside it.
    if pick.is_some() {
        for extra in &sel.extra {
            let (k, p) = split_channel(extra);
            if k == key {
                if let Some(p) = p {
                    out.push(described(Some(p)));
                }
            }
        }
    }
    out.truncate(ivory_ui::recorder::INPUTS);
    out
}

/// Choose the inputs open beside the primary, as channel uids.
pub fn set_extra_inputs(shared: &Shared, uids: Vec<String>) {
    let mut sel = lock(shared);
    if sel.extra != uids {
        sel.extra = uids;
        // The stream has to be rebuilt: how many channels it carries and which
        // ones is decided when it opens.
        sel.settled = false;
    }
}

/// A snapshot of the selection, for the reconciler to read.
///
/// Cloned out rather than handing back a guard: the reconciler goes on to open
/// a device, which takes tens of milliseconds, and holding this lock across
/// that would block the next frame's picker.
pub fn selection(shared: &Shared) -> Selection {
    let sel = lock(shared);
    Selection {
        wanted: sel.wanted.clone(),
        extra: sel.extra.clone(),
        explicit: sel.explicit,
        open: sel.open.clone(),
        open_name: sel.open_name.clone(),
        error: sel.error.clone(),
        settled: sel.settled,
        // Not the reconciler's business: it opens what `wanted` names, and
        // which OTHER rows the picker offers changes nothing about that.
        exposed: Vec::new(),
    }
}

/// Seed the selection from what the settings file remembers.
///
/// Called once at startup. Without it a remembered device is never opened,
/// because the reconciler only ever acts on a difference and `wanted` would
/// start empty — so the app would silently fall back to the system default and
/// look like it had forgotten the choice.
pub fn restore(
    shared: &Shared,
    uid: Option<&str>,
    explicitly_off: bool,
    exposed: &[String],
    extra: &[String],
) {
    let mut sel = lock(shared);
    // `explicitly_off` is what makes "None - record MIDI only" survive a
    // restart. Deriving `explicit` from `uid.is_some()` alone would turn an
    // explicit no into a never-asked on every launch, and open the system
    // microphone for somebody who said not to.
    sel.explicit = uid.is_some() || explicitly_off;
    sel.wanted = uid.map(str::to_owned);
    // Kept even for devices that are not plugged in right now: `list` shows a
    // row only for hardware it can see, and an interface that comes back
    // should come back set up the way it was left.
    sel.exposed = exposed.to_vec();
    // The inputs open beside the first. Kept whole here and filtered where the
    // picks are built, so an interface that comes back comes back with all of
    // them rather than with whichever ones happened to be visible at launch.
    sel.extra = extra.to_vec();
}


#[cfg(test)]
mod tests {
    use super::*;

    fn a_device(uid: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            uid: uid.to_owned(),
            name: name.to_owned(),
            default: false,
        }
    }

    /// **Every ticked input is a row of its own, and they coexist.**
    ///
    /// The owner's requirement in their words: mono 6 and 1/2 and 4/5 must all
    /// show up in the input selector at the SAME time, each named for the
    /// interface and the inputs it holds. The chooser this replaced could hold
    /// exactly one of the three.
    #[test]
    fn every_exposed_input_is_its_own_row() {
        let key = "Scarlett 18i20#0";
        let exposed: Vec<String> = vec![
            channel_uid(key, ChannelPick::Mono(5)),
            channel_uid(key, ChannelPick::Stereo(0, 1)),
            channel_uid(key, ChannelPick::Stereo(3, 4)),
        ];
        let rows = with_exposed(
            vec![
                a_device(key, "Scarlett 18i20"),
                a_device("MacBook Pro Microphone#0", "MacBook Pro Microphone"),
            ],
            &exposed,
        );
        let names: Vec<&str> = rows.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Scarlett 18i20",
                "Scarlett 18i20  -  inputs 1/2",
                "Scarlett 18i20  -  inputs 4/5",
                "Scarlett 18i20  -  input 6",
                "MacBook Pro Microphone",
            ],
            "the picker did not offer all three at once, under the interface"
        );
        // Every row still opens something distinct: the uids are what the
        // reconciler acts on, and two rows sharing one would be two names for
        // the same microphone.
        let mut uids: Vec<&str> = rows.iter().map(|d| d.uid.as_str()).collect();
        let n = uids.len();
        uids.sort_unstable();
        uids.dedup();
        assert_eq!(uids.len(), n, "two rows named the same input");
    }

    /// A remembered input for hardware that is not plugged in shows nothing,
    /// and is not forgotten either.
    ///
    /// Both halves matter: a row for an absent interface is a microphone that
    /// cannot be opened, and dropping the setting because the box was unplugged
    /// would mean setting it up again every time it comes back.
    #[test]
    fn an_absent_interface_contributes_no_rows_and_loses_nothing() {
        let gone = channel_uid("Scarlett 18i20#0", ChannelPick::Stereo(0, 1));
        let rows = with_exposed(
            vec![a_device("MacBook Pro Microphone#0", "MacBook Pro Microphone")],
            std::slice::from_ref(&gone),
        );
        assert_eq!(rows.len(), 1, "a row appeared for hardware that is not here");

        let (_a, shared) = AudioInputs::new();
        restore(&shared, None, false, std::slice::from_ref(&gone), &[]);
        assert_eq!(
            lock(&shared).exposed,
            vec![gone],
            "the setup was thrown away with the cable"
        );
    }

    /// **Unticking the input you are recording leaves a microphone open.**
    ///
    /// The row vanishes from the list the moment the box is cleared, so a
    /// selection left pointing at it is pointing at nothing. Falling back to
    /// the interface itself is the difference between a tidy-up and a silent
    /// take nobody noticed until playback.
    #[test]
    fn unticking_the_open_input_falls_back_to_the_whole_device() {
        use ivory_ui::ports::AudioSetup as _;
        let key = "Scarlett 18i20#0";
        let (_a, shared) = AudioInputs::new();
        let pair = channel_uid(key, ChannelPick::Stereo(0, 1));
        restore(&shared, Some(&pair), false, &[pair.clone()], &[]);

        let mut setup = Setup::new(&shared);
        assert_eq!(setup.exposed(), vec![(0, Some(1))]);

        // Tick a different one and clear the one that is open.
        let left = setup.set_exposed(vec![(5, None)]);
        assert_eq!(left, vec![channel_uid(key, ChannelPick::Mono(5))]);
        assert_eq!(
            lock(&shared).wanted.as_deref(),
            Some(key),
            "the picker was left pointing at a row that no longer exists"
        );
        assert!(lock(&shared).explicit, "the stream will not be reopened");
    }

    /// One interface's boxes are not the other's.
    #[test]
    fn setting_up_one_interface_leaves_the_others_alone() {
        use ivory_ui::ports::AudioSetup as _;
        let other = channel_uid("Behringer UMC1820#0", ChannelPick::Mono(7));
        let (_a, shared) = AudioInputs::new();
        restore(&shared, Some("Scarlett 18i20#0"), false, &[other.clone()], &[]);

        let mut setup = Setup::new(&shared);
        assert!(
            setup.exposed().is_empty(),
            "the Scarlett inherited the Behringer's inputs"
        );
        let all = setup.set_exposed(vec![(0, Some(1))]);
        assert!(
            all.contains(&other),
            "the Behringer's input was forgotten while the Scarlett was set up"
        );
    }

    #[test]
    fn choosing_a_device_does_not_open_it() {
        let (mut a, shared) = AudioInputs::new();
        a.open("Scarlett 2i2#0").expect("recording intent cannot fail");
        assert_eq!(a.current().as_deref(), Some("Scarlett 2i2#0"));
        assert!(
            selection(&shared).is_stale(),
            "the reconciler has to be told there is work"
        );
        assert_eq!(
            a.current_name(),
            None,
            "no name until something has actually been opened"
        );
    }

    #[test]
    fn settling_clears_the_staleness_and_names_the_device() {
        let (a, shared) = AudioInputs::new();
        lock(&shared).wanted = Some("Scarlett 2i2#0".into());
        settle(
            &shared,
            Some("Scarlett 2i2#0".into()),
            Some("Scarlett 2i2".into()),
            None,
        );
        assert!(!selection(&shared).is_stale());
        assert_eq!(a.current_name().as_deref(), Some("Scarlett 2i2"));
    }

    /// A failed open must not look like a successful one, and must not make the
    /// reconciler retry every frame forever — which is what would happen if the
    /// failure left `open` unequal to `wanted`.
    #[test]
    fn a_failed_open_settles_too_and_carries_its_reason() {
        let (_a, shared) = AudioInputs::new();
        lock(&shared).wanted = Some("gone#0".into());
        settle(
            &shared,
            Some("gone#0".into()),
            None,
            Some("the device is not connected".into()),
        );
        let sel = selection(&shared);
        assert!(
            !sel.is_stale(),
            "a failure is a settled state, not a retry loop"
        );
        assert_eq!(sel.open_name, None);
        assert!(sel.error.is_some());
    }

    /// The None row in the picker.
    #[test]
    fn an_empty_uid_selects_nothing_rather_than_a_device_named_empty() {
        let (mut a, _shared) = AudioInputs::new();
        a.open("something").expect("ok");
        a.open("").expect("ok");
        assert_eq!(a.current(), None);
    }

    /// **No chosen input means no input opened — ever.**
    ///
    /// This test used to assert the opposite for the "never opened the picker"
    /// case: it handed back `InputSelection::Default` so the meter would be
    /// live before anybody armed anything. That is a nice idea and it was the
    /// wrong one. "The system default input" is whatever the OS is pointing at,
    /// and on a machine with Loopback or any virtual device installed that is a
    /// mic recording the wrong thing — which is exactly what the owner hit, an
    /// app that kept coming up on a Virtual Mic they had never chosen. It also
    /// raised the microphone prompt at launch for a device nobody asked for.
    ///
    /// So all three cases below agree now, and that is the point: absent is
    /// absent, however it got that way.
    #[test]
    fn no_chosen_input_means_no_input_is_ever_opened() {
        use ivory_record::audio::InputSelection;
        let (mut a, shared) = AudioInputs::new();

        assert_eq!(
            audio_selection(&shared),
            None,
            "never asked must not open a microphone"
        );

        a.open("").expect("ok");
        assert_eq!(audio_selection(&shared), None, "no device means no device");

        // Seeded from settings, both ways round: an explicit "None"...
        let (_b, restarted) = AudioInputs::new();
        restore(&restarted, None, true, &[], &[]);
        assert_eq!(audio_selection(&restarted), None);

        // ...and a file from somebody who never chose. Same answer.
        let (_c, fresh) = AudioInputs::new();
        restore(&fresh, None, false, &[], &[]);
        assert_eq!(audio_selection(&fresh), None);

        // A chosen device is still opened, which is the half that must not
        // regress while fixing the other one.
        let (_d, picked) = AudioInputs::new();
        restore(&picked, Some("Scarlett 2i2#0"), false, &[], &[]);
        assert!(matches!(
            audio_selection(&picked),
            Some(AudioChoice {
                selection: InputSelection::Key(_),
                channels
            }) if channels.is_empty()
        ));
    }

    /// **The uid grammar survives a device name that would break it.**
    ///
    /// The separator has to be a character no interface puts in its own name,
    /// and the obvious candidates all fail: `#` is already taken and escaped by
    /// `DeviceKey`, and `|`, `:` and `@` are all in real product names. A C0
    /// control is in none, which is the argument — and this is the test that
    /// makes it an argument rather than a hope.
    ///
    /// The failure it prevents is quiet: a uid that splits wrong resolves to a
    /// device that does not exist, so the saved microphone silently reverts to
    /// nothing on the next launch and the user re-picks it every session.
    #[test]
    fn a_channel_uid_round_trips_through_a_hostile_device_name() {
        use ivory_record::audio::DeviceKey;
        for name in [
            "Scarlett 18i20 USB",
            "Mic #2",
            "Focusrite | 8",
            "A:B@C",
            "1-2",
            "Built-in Microphone",
        ] {
            let key = DeviceKey::named(name).to_setting();
            // The device's own row carries no channel and must not grow one.
            assert_eq!(split_channel(&key), (key.as_str(), None), "{name}");
            for ch in [0_u16, 3, 17] {
                let uid = channel_uid(&key, ChannelPick::Mono(ch));
                let (back, got) = split_channel(&uid);
                assert_eq!(back, key, "{name} at channel {ch}");
                assert_eq!(got, Some(ChannelPick::Mono(ch)), "{name} at channel {ch}");
                assert_eq!(
                    DeviceKey::from_setting(back),
                    DeviceKey::named(name),
                    "{name} did not survive the round trip via {uid:?}"
                );
            }
        }
    }

    /// **A PipeWire uid resolves to a node, not to a cpal device name.**
    ///
    /// The two grammars share one settings key, so the prefix is what keeps a
    /// saved `alsa_input.usb-Focusrite…` from being looked up as an ALSA device
    /// of that name — which does not exist, so the microphone would silently
    /// revert to nothing on the next launch.
    #[test]
    fn a_pipewire_uid_names_a_node() {
        use ivory_record::audio::InputSelection;
        let node = "alsa_input.usb-Focusrite_Scarlett_Solo_USB_Y771VU50AAEF28-00.pro-input-0";
        let (_a, shared) = AudioInputs::new();
        restore(&shared, Some(&format!("{PIPEWIRE_UID}{node}")), false, &[], &[]);
        let choice = audio_selection(&shared).expect("a device was chosen");
        assert_eq!(
            choice.selection,
            InputSelection::PipewireNode(node.to_owned())
        );
        assert!(choice.channels.is_empty());

        // And with one of its inputs picked out. The channel suffix and the
        // node prefix have to survive each other.
        let (_b, with_ch) = AudioInputs::new();
        restore(
            &with_ch,
            Some(&channel_uid(&format!("{PIPEWIRE_UID}{node}"), ChannelPick::Stereo(1, 2))),
            false,
            &[],
            &[],
        );
        let choice = audio_selection(&with_ch).expect("a device was chosen");
        assert_eq!(
            choice.selection,
            InputSelection::PipewireNode(node.to_owned()),
            "the channel suffix ate the node name"
        );
        assert_eq!(
            choice.channels.iter().collect::<Vec<_>>(),
            vec![ChannelPick::Stereo(1, 2)]
        );
    }

    /// **Several inputs of one interface, in the order they were chosen.**
    ///
    /// The owner's case: a microphone on input 6 and a synth across 4/5, live
    /// at the same time. `wanted` is input 1 and the extras are inputs 2
    /// upward — and that ORDER is load-bearing three times over: it is the
    /// order the picks are built in, the order the capture lays the channels
    /// out in, and the order the desk draws the strips in. All three come from
    /// here so they cannot drift.
    #[test]
    fn several_inputs_of_one_interface_open_together() {
        let key = "Scarlett 18i20#0";
        let mono6 = channel_uid(key, ChannelPick::Mono(5));
        let pair45 = channel_uid(key, ChannelPick::Stereo(3, 4));
        let (_b, shared) = AudioInputs::new();
        restore(&shared, Some(&mono6), false, &[], std::slice::from_ref(&pair45));
        let choice = audio_selection(&shared).expect("a device was chosen");
        assert_eq!(
            choice.channels.iter().collect::<Vec<_>>(),
            vec![ChannelPick::Mono(5), ChannelPick::Stereo(3, 4)],
            "the chosen order is not the order the stream will be laid out in"
        );
        assert_eq!(choice.channels.channels(), 3, "one channel plus two");

        // **A uid from another interface is dropped, not obeyed.** One box is
        // one clock; a leftover from a machine with different hardware would
        // open a channel number against an interface that never had it.
        let (_b2, mixed) = AudioInputs::new();
        let elsewhere = channel_uid("Behringer UMC1820#0", ChannelPick::Mono(7));
        restore(
            &mixed,
            Some(&mono6),
            false,
            &[],
            &[elsewhere, pair45.clone()],
        );
        let choice = audio_selection(&mixed).expect("a device was chosen");
        assert_eq!(
            choice.channels.iter().collect::<Vec<_>>(),
            vec![ChannelPick::Mono(5), ChannelPick::Stereo(3, 4)],
            "an input from another interface was opened"
        );

        // And the names line up with the picks, one strip each, primary first.
        // `open_name` is what the reconciler writes when the device actually
        // opens, and a strip named after a device that did not open would be a
        // lie — so the test says it opened.
        lock(&shared).open_name = Some("Scarlett 18i20".to_owned());
        let named = open_inputs(&shared);
        assert_eq!(named.len(), 2);
        // The interface and the channel come back APART, so the desk can put
        // a short name in front of a label that would never fit whole.
        assert_eq!(named[0].0, "Scarlett 18i20");
        assert_eq!(named[0].1, "6");
        assert_eq!(named[1].0, "Scarlett 18i20", "both are the same box");
        assert_eq!(named[1].1, "4/5");
        assert!(!named[0].2, "a mono input is not stereo");
        assert!(named[1].2, "a pair is stereo");
    }

    /// The same uid twice is one input, not two strips of the same microphone.
    #[test]
    fn one_input_cannot_be_opened_twice() {
        let key = "Scarlett 18i20#0";
        let mono6 = channel_uid(key, ChannelPick::Mono(5));
        let (_b, shared) = AudioInputs::new();
        restore(&shared, Some(&mono6), false, &[], std::slice::from_ref(&mono6));
        let choice = audio_selection(&shared).expect("a device was chosen");
        assert_eq!(choice.channels.len(), 1, "the same input opened twice");
    }

    /// A chosen channel reaches the stream open, with its device intact.
    #[test]
    fn a_chosen_channel_reaches_the_stream_open() {
        use ivory_record::audio::{DeviceKey, InputSelection};
        let (_a, shared) = AudioInputs::new();
        let uid = channel_uid(
            &DeviceKey::named("Scarlett 18i20 USB").to_setting(),
            ChannelPick::Mono(2),
        );
        restore(&shared, Some(&uid), false, &[], &[]);
        let choice = audio_selection(&shared).expect("a device was chosen");
        assert_eq!(
            choice.selection,
            InputSelection::Key(DeviceKey::named("Scarlett 18i20 USB"))
        );
        // Zero-based here; the chooser's label said "input 3".
        assert_eq!(
            choice.channels.iter().collect::<Vec<_>>(),
            vec![ChannelPick::Mono(2)]
        );
    }

    /// A hand-edited channel that is not a number costs the channel, not the
    /// microphone.
    #[test]
    fn a_malformed_channel_still_opens_the_device() {
        use ivory_record::audio::{DeviceKey, InputSelection};
        let (_a, shared) = AudioInputs::new();
        restore(&shared, Some(&format!("Scarlett{CHANNEL_SEP}left")), false, &[], &[]);
        let choice = audio_selection(&shared).expect("the device still opens");
        assert_eq!(
            choice.selection,
            InputSelection::Key(DeviceKey::named("Scarlett"))
        );
        assert!(choice.channels.is_empty());
    }

    /// A remembered device has to be re-opened at launch, or the app looks like
    /// it forgot the choice and silently uses the default instead.
    #[test]
    fn a_remembered_device_is_stale_at_startup_so_it_gets_opened() {
        use ivory_record::audio::InputSelection;
        let (_a, shared) = AudioInputs::new();
        restore(&shared, Some("Scarlett 2i2#0"), false, &[], &[]);
        assert!(selection(&shared).is_stale());
        assert!(matches!(
            audio_selection(&shared),
            Some(AudioChoice {
                selection: InputSelection::Key(_),
                channels
            }) if channels.is_empty()
        ));
    }

    /// The initial state and the settled result of choosing None look identical
    /// (`wanted == open == None`), so without the `settled` flag the reconciler
    /// either never runs or runs forever. It must run exactly once.
    #[test]
    fn a_never_reconciled_selection_is_stale_and_a_settled_one_is_not() {
        let (_a, shared) = AudioInputs::new();
        assert!(
            selection(&shared).is_stale(),
            "nothing has been opened yet, so there is work to do"
        );
        settle(&shared, None, None, None);
        assert!(!selection(&shared).is_stale(), "and now there is not");
    }

    /// The camera has the same shape, and the same None-means-close rule — with
    /// the light on the front of the machine as the consequence of getting it
    /// wrong.
    #[test]
    fn a_camera_selection_behaves_like_an_audio_one() {
        let (mut c, shared, denied) = Cameras::new();
        assert_eq!(c.current(), None);
        c.open("0x1400000046d0825").expect("ok");
        assert!(selection(&shared).is_stale());
        c.open("").expect("ok");
        assert_eq!(c.current(), None, "None must reach the reconciler as None");
        assert!(denied.lock().expect("lock").is_none());
    }
}
