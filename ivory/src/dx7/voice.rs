//! A DX7 voice: the 155 numbers that are a patch, and the 128 bytes they
//! arrive in.
//!
//! # The format
//!
//! A cartridge is a 4104-byte SysEx bulk dump: `F0 43 0n 09 20 00`, 4096 bytes
//! of packed voice data, a checksum, `F7`. The 4096 is 32 voices of 128 bytes,
//! and each of those unpacks to 155 parameters.
//!
//! **Packed and unpacked are different layouts, not the same one compressed.**
//! The packed form squeezes pairs of small fields into shared bytes: left and
//! right curve into one, rate scaling and detune into another. Getting a bit
//! range wrong does not fail — it produces a patch that plays and sounds wrong,
//! which is the worst kind of bug to find by ear. Every field below names its
//! bits.
//!
//! # Operator order
//!
//! **The SysEx stores operator 6 first.** Byte 0 of a voice is OP6's first
//! envelope rate, and OP1 is last. This module unpacks into `ops[0] == OP1`,
//! because every algorithm diagram ever printed numbers them that way and code
//! that says `ops[0]` while meaning OP6 is code nobody can check against the
//! documentation.

/// One operator's parameters, as the DX7 stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Op {
    /// Envelope rates and levels, 0..=99 each. R1/L1 is the attack target.
    pub rate: [u8; 4],
    pub level: [u8; 4],
    /// Keyboard level scaling: where the curves meet, how deep each side goes,
    /// and which of the four shapes each side uses (0 -lin, 1 -exp, 2 +exp,
    /// 3 +lin).
    pub break_point: u8,
    pub left_depth: u8,
    pub right_depth: u8,
    pub left_curve: u8,
    pub right_curve: u8,
    /// How much higher notes shorten the envelope, 0..=7.
    pub rate_scaling: u8,
    /// Sensitivity to the LFO's amplitude modulation, 0..=3.
    pub amp_mod_sens: u8,
    /// Sensitivity to key velocity, 0..=7.
    pub vel_sens: u8,
    /// 0..=99. The operator's own level, before any scaling.
    pub output_level: u8,
    /// 0 = a ratio of the note's pitch, 1 = a fixed frequency in Hz.
    pub fixed: bool,
    /// 0..=31 coarse, 0..=99 fine. Together they make the ratio, or the decade.
    pub coarse: u8,
    pub fine: u8,
    /// 0..=14, with 7 meaning none. A few cents either way, which is what stops
    /// two operators at the same ratio sounding like one.
    pub detune: u8,
}

/// A whole patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    /// `ops[0]` is OP1. See the module docs: the SysEx has this backwards.
    pub ops: [Op; 6],
    pub pitch_rate: [u8; 4],
    pub pitch_level: [u8; 4],
    /// 0..=31, the routing. Printed as 1..=32 everywhere a human reads it.
    pub algorithm: u8,
    /// 0..=7, applied to the one operator the algorithm feeds back into.
    pub feedback: u8,
    pub osc_sync: bool,
    pub lfo_speed: u8,
    pub lfo_delay: u8,
    pub lfo_pitch_depth: u8,
    pub lfo_amp_depth: u8,
    pub lfo_sync: bool,
    /// 0..=5: triangle, saw down, saw up, square, sine, sample and hold.
    pub lfo_wave: u8,
    /// 0..=7, how much the LFO's pitch depth reaches this voice at all.
    pub pitch_mod_sens: u8,
    /// 0..=48, where 24 is no transposition.
    pub transpose: u8,
    /// Ten characters, space-padded, as stored. Not trimmed here: what the
    /// cartridge says is what the picker shows.
    pub name: [u8; 10],
}

impl Voice {
    /// The patch a fresh install plays.
    ///
    /// **Written here rather than shipped as a cartridge**, and that is a
    /// licensing decision as much as a size one. The factory ROMs are Yamaha's
    /// and the banks people trade are their authors'; a patch typed into this
    /// file is unambiguously the app's to give away, and it is 155 numbers.
    ///
    /// A tine electric piano on algorithm 1, which is two carriers each with
    /// one modulator. OP2 at fourteen times the fundamental is the strike and
    /// dies in a fifth of a second; OP4 at the fundamental thickens the body.
    /// The carriers decay to nothing rather than sustaining, because that is
    /// what a struck instrument does and a synth pad default would be the wrong
    /// first impression for an app that draws a piano.
    pub fn electric_piano() -> Self {
        let carrier = |level: u8| Op {
            rate: [99, 62, 21, 68],
            level: [99, 92, 0, 0],
            output_level: level,
            coarse: 1,
            fine: 0,
            detune: 7,
            rate_scaling: 3,
            vel_sens: 3,
            break_point: 39,
            ..Op::default()
        };
        // A modulator's envelope is what the ear hears as brightness, so the
        // strike falls far faster than the note it is on.
        let tine = Op {
            rate: [99, 70, 40, 78],
            level: [99, 26, 0, 0],
            output_level: 74,
            coarse: 14,
            fine: 0,
            detune: 7,
            rate_scaling: 4,
            vel_sens: 6,
            ..Op::default()
        };
        let body = Op {
            rate: [99, 58, 26, 68],
            level: [99, 68, 0, 0],
            output_level: 58,
            coarse: 1,
            fine: 0,
            detune: 9,
            rate_scaling: 2,
            vel_sens: 2,
            ..Op::default()
        };
        Self {
            // Algorithm 1: 2 modulates 1, and 4 modulates 3 through the stack.
            algorithm: 0,
            feedback: 3,
            ops: [
                carrier(99), // OP1, heard
                tine,        // OP2 -> OP1
                carrier(82), // OP3, heard
                body,        // OP4 -> OP3
                Op::default(),
                Op::default(),
            ],
            name: *b"E.PIANO  1",
            ..Self::silent()
        }
    }

    /// Every parameter at rest. Silent, and the base every other patch is
    /// built from.
    fn silent() -> Self {
        Self {
            ops: [Op::default(); 6],
            pitch_rate: [99; 4],
            pitch_level: [50; 4],
            algorithm: 0,
            feedback: 0,
            osc_sync: true,
            lfo_speed: 35,
            lfo_delay: 0,
            lfo_pitch_depth: 0,
            lfo_amp_depth: 0,
            lfo_sync: true,
            lfo_wave: 0,
            pitch_mod_sens: 0,
            transpose: 24,
            name: *b"INIT VOICE",
        }
    }
}

impl Default for Voice {
    /// **A sound, not silence.** A default of all zeros is a patch that plays
    /// nothing, which is what a fresh install did before there was a patch to
    /// give it.
    fn default() -> Self {
        Self::electric_piano()
    }
}

impl Voice {
    /// The name, trimmed, with anything unprintable replaced.
    ///
    /// Cartridges in the wild carry high-bit bytes and control characters in
    /// the name field; a picker that renders those is a picker full of boxes.
    pub fn display_name(&self) -> String {
        let s: String = self
            .name
            .iter()
            .map(|b| {
                let c = *b & 0x7f;
                if (0x20..0x7f).contains(&c) {
                    c as char
                } else {
                    ' '
                }
            })
            .collect();
        let t = s.trim();
        if t.is_empty() {
            "(unnamed)".to_owned()
        } else {
            t.to_owned()
        }
    }

    /// Unpack one voice from its 128 packed bytes.
    ///
    /// Never fails: every field is masked into its own range, so a corrupt
    /// cartridge produces a playable patch rather than an error. The DX7 itself
    /// is this forgiving, and a bank that refuses to open because one voice in
    /// it is damaged is worse than one odd sound.
    pub fn unpack(p: &[u8; 128]) -> Self {
        let mut v = Self::default();
        for i in 0..6 {
            // OP6 first in the file, OP1 first in `ops`.
            let b = &p[i * 17..i * 17 + 17];
            let op = &mut v.ops[5 - i];
            for k in 0..4 {
                op.rate[k] = b[k] & 0x7f;
                op.level[k] = b[4 + k] & 0x7f;
            }
            op.break_point = b[8] & 0x7f;
            op.left_depth = b[9] & 0x7f;
            op.right_depth = b[10] & 0x7f;
            // Byte 11: bits 0-1 left curve, bits 2-3 right curve.
            op.left_curve = b[11] & 0x03;
            op.right_curve = (b[11] >> 2) & 0x03;
            // Byte 12: bits 0-2 rate scaling, bits 3-6 detune.
            op.rate_scaling = b[12] & 0x07;
            op.detune = (b[12] >> 3) & 0x0f;
            // Byte 13: bits 0-1 amplitude mod sensitivity, bits 2-4 velocity.
            op.amp_mod_sens = b[13] & 0x03;
            op.vel_sens = (b[13] >> 2) & 0x07;
            op.output_level = b[14] & 0x7f;
            // Byte 15: bit 0 oscillator mode, bits 1-5 coarse frequency.
            op.fixed = b[15] & 0x01 != 0;
            op.coarse = (b[15] >> 1) & 0x1f;
            op.fine = b[16] & 0x7f;
        }
        for k in 0..4 {
            v.pitch_rate[k] = p[102 + k] & 0x7f;
            v.pitch_level[k] = p[106 + k] & 0x7f;
        }
        v.algorithm = p[110] & 0x1f;
        // Byte 111: bits 0-2 feedback, bit 3 oscillator key sync.
        v.feedback = p[111] & 0x07;
        v.osc_sync = (p[111] >> 3) & 0x01 != 0;
        v.lfo_speed = p[112] & 0x7f;
        v.lfo_delay = p[113] & 0x7f;
        v.lfo_pitch_depth = p[114] & 0x7f;
        v.lfo_amp_depth = p[115] & 0x7f;
        // Byte 116: bit 0 sync, bits 1-3 wave, bits 4-6 pitch mod sensitivity.
        v.lfo_sync = p[116] & 0x01 != 0;
        v.lfo_wave = (p[116] >> 1) & 0x07;
        v.pitch_mod_sens = (p[116] >> 4) & 0x07;
        v.transpose = p[117] & 0x3f;
        v.name.copy_from_slice(&p[118..128]);
        v
    }

    /// The inverse: 155 parameters back into the 128 packed bytes.
    ///
    /// **So a patch made here is a patch a DX7 can play.** An editor that could
    /// only keep its work in this app's own settings would be a toy; what comes
    /// out of `Cartridge::to_bytes` is an ordinary SysEx bank, which loads into
    /// Dexed, into a TX802, and into the machine itself.
    ///
    /// Every field is masked on the way out as well as on the way in. A value
    /// out of range cannot corrupt a NEIGHBOURING parameter here — the two that
    /// share a byte would otherwise bleed into each other, and the symptom is a
    /// patch that changes its detune when you edit its rate scaling.
    pub fn pack(&self) -> [u8; 128] {
        let mut p = [0u8; 128];
        for i in 0..6 {
            // OP6 first in the file, OP1 first in `ops` — see the module docs.
            let op = &self.ops[5 - i];
            let b = &mut p[i * 17..i * 17 + 17];
            for k in 0..4 {
                b[k] = op.rate[k] & 0x7f;
                b[4 + k] = op.level[k] & 0x7f;
            }
            b[8] = op.break_point & 0x7f;
            b[9] = op.left_depth & 0x7f;
            b[10] = op.right_depth & 0x7f;
            b[11] = (op.left_curve & 0x03) | ((op.right_curve & 0x03) << 2);
            b[12] = (op.rate_scaling & 0x07) | ((op.detune & 0x0f) << 3);
            b[13] = (op.amp_mod_sens & 0x03) | ((op.vel_sens & 0x07) << 2);
            b[14] = op.output_level & 0x7f;
            b[15] = u8::from(op.fixed) | ((op.coarse & 0x1f) << 1);
            b[16] = op.fine & 0x7f;
        }
        for k in 0..4 {
            p[102 + k] = self.pitch_rate[k] & 0x7f;
            p[106 + k] = self.pitch_level[k] & 0x7f;
        }
        p[110] = self.algorithm & 0x1f;
        p[111] = (self.feedback & 0x07) | (u8::from(self.osc_sync) << 3);
        p[112] = self.lfo_speed & 0x7f;
        p[113] = self.lfo_delay & 0x7f;
        p[114] = self.lfo_pitch_depth & 0x7f;
        p[115] = self.lfo_amp_depth & 0x7f;
        p[116] = u8::from(self.lfo_sync)
            | ((self.lfo_wave & 0x07) << 1)
            | ((self.pitch_mod_sens & 0x07) << 4);
        p[117] = self.transpose & 0x3f;
        // The name as stored, with anything a DX7 cannot show made a space.
        // Seven-bit ASCII is what the format carries, and a high byte here is
        // a name that reads as garbage on the instrument.
        for (dst, src) in p[118..128].iter_mut().zip(self.name) {
            let c = src & 0x7f;
            *dst = if (0x20..0x7f).contains(&c) { c } else { b' ' };
        }
        p
    }

    /// Set the name from text, padded and trimmed to the ten bytes the format
    /// has room for.
    pub fn set_name(&mut self, text: &str) {
        let mut out = [b' '; 10];
        for (dst, c) in out.iter_mut().zip(text.chars()) {
            let b = c as u32;
            *dst = if (0x20..0x7f).contains(&b) { b as u8 } else { b' ' };
        }
        self.name = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bit fields are the whole risk in this file: a wrong range plays and
    /// sounds wrong rather than failing. This packs known values into the
    /// documented positions and reads them back.
    #[test]
    fn every_packed_bit_field_lands_where_the_format_says() {
        let mut p = [0u8; 128];
        // OP1 is the LAST operator block in the file.
        let b = 5 * 17;
        p[b] = 31; // R1
        p[b + 4] = 99; // L1
        p[b + 8] = 60; // break point
        p[b + 11] = 0b0000_1001; // left curve 1, right curve 2
        p[b + 12] = 0b0111_1101; // rate scaling 5, detune 15 -> masked to 15
        p[b + 13] = 0b0001_1110; // amp mod 2, velocity 7
        p[b + 14] = 85; // output level
        p[b + 15] = 0b0011_1011; // fixed, coarse 29
        p[b + 16] = 42; // fine
        p[110] = 17; // algorithm 18 as printed
        p[111] = 0b0000_1101; // feedback 5, sync on
        p[116] = 0b0101_0111; // sync on, wave 3, pitch mod sens 5
        p[117] = 24;
        p[118..128].copy_from_slice(b"TEST NAME ");

        let v = Voice::unpack(&p);
        let op = v.ops[0];
        assert_eq!(op.rate[0], 31);
        assert_eq!(op.level[0], 99);
        assert_eq!(op.break_point, 60);
        assert_eq!((op.left_curve, op.right_curve), (1, 2));
        assert_eq!((op.rate_scaling, op.detune), (5, 15));
        assert_eq!((op.amp_mod_sens, op.vel_sens), (2, 7));
        assert_eq!(op.output_level, 85);
        assert!(op.fixed);
        assert_eq!((op.coarse, op.fine), (29, 42));
        assert_eq!(v.algorithm, 17);
        assert_eq!((v.feedback, v.osc_sync), (5, true));
        assert_eq!((v.lfo_wave, v.pitch_mod_sens), (3, 5));
        assert_eq!(v.display_name(), "TEST NAME");
    }

    /// OP1 is the last block in the file and the first in the array. Getting
    /// this backwards transposes every algorithm silently.
    #[test]
    fn operator_one_is_the_last_block_in_the_file() {
        let mut p = [0u8; 128];
        // Byte 14 of a block is that operator's output level, and a block is
        // seventeen bytes. Named rather than written out, so the two lines
        // below differ only in which BLOCK they name.
        let level_of = |block: usize| block * 17 + 14;
        p[level_of(0)] = 11; // first block in the file is OP6
        p[level_of(5)] = 66; // last block is OP1
        let v = Voice::unpack(&p);
        assert_eq!(v.ops[0].output_level, 66, "ops[0] is not OP1");
        assert_eq!(v.ops[5].output_level, 11, "ops[5] is not OP6");
    }

    /// **Pack is the exact inverse of unpack, for every patch there is.**
    ///
    /// Driven from random bytes rather than from a handful of chosen values:
    /// the risk in this file is a bit field packed into the wrong range, and a
    /// range that is wrong for one value is usually right for zero. Unpacking
    /// masks everything, so `unpack -> pack` must reproduce the masked bytes
    /// exactly.
    #[test]
    fn packing_is_the_inverse_of_unpacking() {
        // A cheap deterministic sequence — this needs coverage, not entropy,
        // and a test that depends on `rand` is a test that depends on `rand`.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 33) as u8
        };
        for _ in 0..200 {
            let mut raw = [0u8; 128];
            for b in &mut raw {
                *b = next();
            }
            // Names are text in the format, and `pack` says so by replacing
            // anything unprintable with a space. Compare like with like.
            for b in &mut raw[118..128] {
                *b = b' ' + (*b % 60);
            }
            let v = Voice::unpack(&raw);
            let out = v.pack();
            // The masked form of the input: what `unpack` could actually have
            // seen. Unused bits are not information and are not preserved.
            let want = Voice::unpack(&out).pack();
            assert_eq!(out, want, "a second round trip changed the bytes");
            assert_eq!(
                Voice::unpack(&out),
                v,
                "the packed patch did not read back as itself"
            );
        }
    }

    /// A name is ten bytes of printable ASCII, whatever it is set from.
    #[test]
    fn a_name_is_padded_trimmed_and_printable() {
        let mut v = Voice::default();
        v.set_name("Rhodes");
        assert_eq!(v.display_name(), "Rhodes");
        assert_eq!(v.name.len(), 10);

        v.set_name("a very long patch name indeed");
        assert_eq!(v.display_name(), "a very lon");

        v.set_name("caf\u{e9}\u{301}\n\t");
        assert!(
            v.name.iter().all(|b| (0x20..0x7f).contains(b)),
            "an unprintable byte reached the name"
        );
    }

    /// A name field full of rubbish must not reach the picker as rubbish.
    #[test]
    fn an_unprintable_name_is_still_a_name() {
        let mut p = [0u8; 128];
        p[118..128].copy_from_slice(&[0xff, 0x01, b'O', b'K', 0x00, 0x7f, b' ', b' ', b' ', b' ']);
        assert_eq!(Voice::unpack(&p).display_name(), "OK");
        p[118..128].copy_from_slice(&[0u8; 10]);
        assert_eq!(Voice::unpack(&p).display_name(), "(unnamed)");
    }
}
