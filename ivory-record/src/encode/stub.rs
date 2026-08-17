//! The encoder on platforms that do not have one yet.
//!
//! Refuses rather than pretends. `Encoder::create` returning `Err` is what the
//! Export dialog's video gate reads, so a Windows or Linux build says "this
//! build cannot write video" instead of running a take that produces no `.mp4`
//! and no explanation.
//!
//! RECORDER-PLAN §7 specifies Media Foundation for Windows and openh264 into a
//! `.mov` with LPCM for Linux. Neither is written.

use crate::clock::Nanos;

const NO_ENCODER: &str = "this build cannot write video yet — audio and MIDI \
                          are still recorded";

pub struct Encoder;

impl Encoder {
    pub fn create(_path: &std::path::Path, _spec: super::VideoSpec) -> Result<Self, String> {
        Err(NO_ENCODER.to_owned())
    }

    pub fn push(&mut self, _bgra: &[u8], _pts_ns: Nanos) -> Result<(), String> {
        Err(NO_ENCODER.to_owned())
    }

    pub fn out_of_order(&self) -> u64 {
        0
    }

    pub fn dropped_not_ready(&self) -> u64 {
        0
    }

    pub fn frames_written(&self) -> u64 {
        0
    }

    pub fn finish(self) -> Result<(), String> {
        Err(NO_ENCODER.to_owned())
    }
}

pub fn mux(_m: &super::Mux) -> Result<(), String> {
    Err(NO_ENCODER.to_owned())
}
