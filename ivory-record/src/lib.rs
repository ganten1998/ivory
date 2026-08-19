//! Tangent's recorder: the clock, and the files a take produces.
//!
//! The plan this implements is `docs/RECORDER-PLAN.md`. Read §3 before touching
//! `clock`, and §7 before touching `smf`.
//!
//! It began as **step 1**: the timebase and the MIDI file, both pure arithmetic
//! with no devices in them. That was deliberate — it is the half of the feature
//! where the hard correctness problems live (drift, tick rounding, hanging
//! notes, out-of-order arrivals) and the half that can be tested exhaustively in
//! about a second with no camera, no audio interface and no MIDI keyboard
//! attached. Every module added since keeps that split: the policy and the
//! arithmetic are pure functions with tests, and the part that must touch a
//! device is kept small enough to read in one sitting.
//!
//! Here now: the clock, the MIDI file, the WAV, the take directory, the mixer,
//! audio capture, and camera capture (macOS; Windows and Linux stub out).
//! Still to come, in order: encode and mux, the compositor, then the plugin
//! host.

pub mod audio;
pub mod camera;
pub mod clock;
pub mod decode;
pub mod encode;
pub mod graph;
pub mod smf;
pub mod take;
pub mod wav;

pub use camera::{
    cameras, open_camera, CameraError, CameraInfo, CameraStream, Format, FormatWish, Frame,
    FrameReader, PermissionStatus,
};
pub use clock::{Nanos, RateFit, SourceClock, Timeline};
pub use graph::{AudioSource, MixMeters, MixSpec, Mixer, SourceMode, SourceRole};
pub use smf::{Captured, MidiTake, PPQ};
pub use take::{Manifest, Take, TakeError, WallTime};
pub use wav::{Bext, SampleFormat, WavSpec, WavWriter};
