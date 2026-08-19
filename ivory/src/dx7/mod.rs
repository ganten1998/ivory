//! A DX7, in Tangent.
//!
//! Six-operator FM with the thirty-two algorithms, playing patches read from
//! real `.syx` cartridges. See `sysex.rs` for the file format, `voice.rs` for
//! the 155 parameters that are a patch, and `synth.rs` for what makes the
//! sound.
//!
//! **Why this and not samples.** A patch is 128 bytes. Thirty-two of them are
//! four kilobytes, which is less than one note of a sampled piano, and they are
//! an instrument rather than a recording of one: every note is right at every
//! pitch and every velocity, with no loop points and no zone boundaries.

pub mod algorithms;
pub mod edit;
pub mod synth;
pub mod sysex;
pub mod voice;

pub use synth::Dx7;
pub use sysex::Cartridge;
pub use voice::{Op, Voice};
