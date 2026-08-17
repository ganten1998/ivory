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
    /// user chose *None — record MIDI only*" are opposite instructions, and
    /// mapping both to `None` meant picking None opened the built-in
    /// microphone and showed its name in the band.
    pub wanted: Option<String>,
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
        found
            .into_iter()
            .map(|d| DeviceInfo {
                // The uid is the name-plus-occurrence key, not the bare name:
                // `DeviceKey` exists because two identical interfaces report
                // the same string and cpal offers no way to compare devices.
                uid: d.key.to_setting(),
                name: d.key.to_string(),
                default: d.is_default,
            })
            .collect()
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
        lock(&self.0).open_name.clone()
    }
}

/// The selection as `ivory-record` wants it, or `None` for "open nothing".
///
/// Never-chosen becomes the host's own default, so somebody who has never
/// opened the picker still gets a live meter — that is the entire point of the
/// meter being live before arming. Explicitly-chosen-None becomes `None`, and
/// the caller closes the device.
pub fn audio_selection(shared: &Shared) -> Option<ivory_record::audio::InputSelection> {
    use ivory_record::audio::{DeviceKey, InputSelection};
    let sel = lock(shared);
    match sel.wanted.as_deref() {
        Some(uid) => Some(InputSelection::Key(DeviceKey::from_setting(uid))),
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

/// A snapshot of the selection, for the reconciler to read.
///
/// Cloned out rather than handing back a guard: the reconciler goes on to open
/// a device, which takes tens of milliseconds, and holding this lock across
/// that would block the next frame's picker.
pub fn selection(shared: &Shared) -> Selection {
    let sel = lock(shared);
    Selection {
        wanted: sel.wanted.clone(),
        explicit: sel.explicit,
        open: sel.open.clone(),
        open_name: sel.open_name.clone(),
        error: sel.error.clone(),
        settled: sel.settled,
    }
}

/// Seed the selection from what the settings file remembers.
///
/// Called once at startup. Without it a remembered device is never opened,
/// because the reconciler only ever acts on a difference and `wanted` would
/// start empty — so the app would silently fall back to the system default and
/// look like it had forgotten the choice.
pub fn restore(shared: &Shared, uid: Option<&str>, explicitly_off: bool) {
    let mut sel = lock(shared);
    // `explicitly_off` is what makes "None — record MIDI only" survive a
    // restart. Deriving `explicit` from `uid.is_some()` alone would turn an
    // explicit no into a never-asked on every launch, and open the system
    // microphone for somebody who said not to.
    sel.explicit = uid.is_some() || explicitly_off;
    sel.wanted = uid.map(str::to_owned);
}


#[cfg(test)]
mod tests {
    use super::*;

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
        restore(&restarted, None, true);
        assert_eq!(audio_selection(&restarted), None);

        // ...and a file from somebody who never chose. Same answer.
        let (_c, fresh) = AudioInputs::new();
        restore(&fresh, None, false);
        assert_eq!(audio_selection(&fresh), None);

        // A chosen device is still opened, which is the half that must not
        // regress while fixing the other one.
        let (_d, picked) = AudioInputs::new();
        restore(&picked, Some("Scarlett 2i2#0"), false);
        assert!(matches!(
            audio_selection(&picked),
            Some(InputSelection::Key(_))
        ));
    }

    /// A remembered device has to be re-opened at launch, or the app looks like
    /// it forgot the choice and silently uses the default instead.
    #[test]
    fn a_remembered_device_is_stale_at_startup_so_it_gets_opened() {
        use ivory_record::audio::InputSelection;
        let (_a, shared) = AudioInputs::new();
        restore(&shared, Some("Scarlett 2i2#0"), false);
        assert!(selection(&shared).is_stale());
        assert!(matches!(
            audio_selection(&shared),
            Some(InputSelection::Key(_))
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
