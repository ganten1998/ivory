//! The camera backend for platforms that do not have one yet.
//!
//! Windows (`IMFSourceReader` on a worker thread, `MFSampleExtension_DeviceTimestamp`
//! for the stamp) and Linux (`linuxvideo`, `v4l2_buffer.timestamp` checked
//! against `V4L2_BUF_FLAG_TSTAMP_SRC_MASK`) are RECORDER-PLAN §4 work. Each is a
//! file beside this one implementing the same two functions and
//! [`VideoSource`](super::VideoSource); nothing above the seam changes.
//!
//! **This exists so the crate builds on every target from the first day rather
//! than the last one.** `ivory-record` is cross-compiled — Windows through
//! `cargo xwin` from a Mac — and a `cfg`-shaped hole where the camera should be
//! is how a release discovers on the build box that the Windows binary never
//! compiled. The UI gets [`CameraError::Unsupported`] and can say "not on
//! Windows yet", which is a true sentence, rather than a build failure or a
//! silently absent Recorder band.

use std::sync::Arc;

use super::{
    CameraError, CameraInfo, CameraStats, FormatWish, FrameSlot, PermissionStatus, VideoSource,
};
use crate::audio::Timebase;

pub(super) fn permission_status() -> PermissionStatus {
    PermissionStatus::NotApplicable
}

/// No cameras, and that is not an error: an empty list is the honest answer for
/// a platform with no backend, and it lets the picker render "no cameras" rather
/// than an error banner the user cannot act on.
pub(super) fn cameras() -> Result<Vec<CameraInfo>, CameraError> {
    Ok(Vec::new())
}

/// Always [`CameraError::Unsupported`].
///
/// Reached only if a caller passes a UID that no enumeration on this platform
/// could have produced — a settings file written on a Mac and synced, most
/// likely — so it must be an error the UI can print, not a panic.
pub(super) fn open(
    _uid: &str,
    _wish: &FormatWish,
    _timebase: Timebase,
    _slot: Arc<FrameSlot>,
    _stats: Arc<CameraStats>,
) -> Result<Box<dyn VideoSource>, CameraError> {
    Err(CameraError::Unsupported)
}
