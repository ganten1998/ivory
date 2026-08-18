//! Reading DX7 cartridges: a `.syx` file in, thirty-two patches out.
//!
//! # What a cartridge is
//!
//! `F0 43 0n 09 20 00`, then 4096 bytes of packed voice data, a checksum, and
//! `F7`. 4104 bytes exactly. The 4096 is 32 voices of 128 packed bytes each;
//! `voice.rs` unpacks those.
//!
//! # Leniency, on purpose
//!
//! **A bad checksum is a warning, not a refusal.** Of ten and a half thousand
//! cartridges in the wild, several hundred fail their own checksum and play
//! perfectly on real hardware: they were assembled by editors that did not
//! bother, or trimmed and re-saved. Refusing them would reject working music
//! to enforce a byte nobody reads.
//!
//! What IS refused is a file that is not a cartridge at all, because loading
//! one produces thirty-two patches of noise and no explanation.
//!
//! # What is not supported, and why that is fine
//!
//! A single-voice dump (163 bytes, `F0 43 0n 00 01 1B`) is a different layout:
//! 155 unpacked parameters rather than 128 packed ones. Cartridges are what
//! people collect and trade, and supporting one format properly is better than
//! two badly. The error says which one it got.

use super::voice::Voice;

/// A cartridge: thirty-two patches, in the order the DX7 numbers them.
#[derive(Debug, Clone)]
pub struct Cartridge {
    pub voices: Vec<Voice>,
    /// What the file called itself, for the picker. The format has no bank
    /// name, so this is the file's own stem.
    pub name: String,
    /// True when the checksum matched. Loaded either way; worth reporting.
    pub checksum_ok: bool,
}

/// The exact length of a 32-voice bulk dump.
pub const BULK_LEN: usize = 4104;
const HEADER: [u8; 4] = [0xF0, 0x43, 0x00, 0x09];
const VOICES: usize = 32;
const PACKED: usize = 128;

impl Cartridge {
    /// Parse a cartridge from the bytes of a `.syx` file.
    ///
    /// `name` is what to call it; the format carries no bank name of its own.
    pub fn parse(bytes: &[u8], name: &str) -> Result<Self, String> {
        // Some files carry trailing rubbish, and some editors write a second
        // dump after the first. The first 4104 bytes are the cartridge.
        if bytes.len() < BULK_LEN {
            return Err(format!(
                "{} bytes is not a DX7 cartridge; a 32-voice bank is {BULK_LEN}",
                bytes.len()
            ));
        }
        let d = &bytes[..BULK_LEN];
        // Byte 2 is the channel and varies; byte 3 is the format and must be 9
        // for a bulk dump. A single-voice dump has 0 there, which is worth
        // saying out loud because it is the other thing people have.
        if d[0] != HEADER[0] || d[1] != HEADER[1] {
            return Err("not a SysEx file: it does not begin F0 43".to_owned());
        }
        if d[3] != HEADER[3] {
            return Err(if d[3] == 0 {
                "this is a single-voice dump, not a 32-voice cartridge".to_owned()
            } else {
                format!("SysEx format {} is not a DX7 cartridge", d[3])
            });
        }
        if d[BULK_LEN - 1] != 0xF7 {
            return Err("the cartridge does not end with F7".to_owned());
        }

        let body = &d[6..6 + VOICES * PACKED];
        // Seven-bit two's complement over the data, which is what the DX7
        // writes and what most editors reproduce.
        let sum: u32 = body.iter().map(|b| u32::from(*b)).sum();
        let checksum_ok = ((sum.wrapping_neg()) & 0x7f) as u8 == d[BULK_LEN - 2];

        let mut voices = Vec::with_capacity(VOICES);
        for i in 0..VOICES {
            let mut packed = [0u8; PACKED];
            packed.copy_from_slice(&body[i * PACKED..(i + 1) * PACKED]);
            voices.push(Voice::unpack(&packed));
        }
        Ok(Self {
            voices,
            name: name.to_owned(),
            checksum_ok,
        })
    }

    /// Read one from disk.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .map_or_else(|| "cartridge".to_owned(), |s| s.to_string_lossy().into_owned());
        Self::parse(&bytes, &name)
    }

    /// Every patch's name, for a picker.
    pub fn names(&self) -> Vec<String> {
        self.voices.iter().map(Voice::display_name).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cartridge assembled by hand, so the parser is tested against the
    /// format rather than against whatever happens to be on this disk.
    fn synthetic(good_checksum: bool) -> Vec<u8> {
        let mut d = vec![0u8; BULK_LEN];
        d[..6].copy_from_slice(&[0xF0, 0x43, 0x00, 0x09, 0x20, 0x00]);
        d[BULK_LEN - 1] = 0xF7;
        for i in 0..VOICES {
            let at = 6 + i * PACKED;
            // A name per voice, so order can be asserted.
            let name = format!("VOICE {i:<4}");
            d[at + 118..at + 128].copy_from_slice(&name.as_bytes()[..10]);
            d[at + 110] = (i % 32) as u8; // algorithm
        }
        let sum: u32 = d[6..6 + VOICES * PACKED].iter().map(|b| u32::from(*b)).sum();
        d[BULK_LEN - 2] = if good_checksum {
            ((sum.wrapping_neg()) & 0x7f) as u8
        } else {
            0x01
        };
        d
    }

    #[test]
    fn a_cartridge_is_thirty_two_voices_in_order() {
        let c = Cartridge::parse(&synthetic(true), "bank").expect("a valid cartridge");
        assert_eq!(c.voices.len(), 32);
        assert!(c.checksum_ok);
        assert_eq!(c.names()[0], "VOICE 0");
        assert_eq!(c.names()[31], "VOICE 31");
        assert_eq!(c.voices[17].algorithm, 17);
    }

    /// **A bad checksum loads.** Several hundred real cartridges fail their own
    /// and play fine on hardware; refusing them rejects working music over a
    /// byte nobody reads.
    #[test]
    fn a_bad_checksum_is_reported_and_not_refused() {
        let c = Cartridge::parse(&synthetic(false), "bank").expect("still loads");
        assert!(!c.checksum_ok);
        assert_eq!(c.voices.len(), 32);
    }

    /// The three ways a file can not be a cartridge, each with its own words.
    #[test]
    fn what_is_not_a_cartridge_says_so() {
        assert!(Cartridge::parse(&[0xF0, 0x43], "x")
            .unwrap_err()
            .contains("not a DX7 cartridge"));

        let mut single = vec![0u8; BULK_LEN];
        single[..4].copy_from_slice(&[0xF0, 0x43, 0x00, 0x00]);
        single[BULK_LEN - 1] = 0xF7;
        assert!(Cartridge::parse(&single, "x")
            .unwrap_err()
            .contains("single-voice"));

        let mut wrong = synthetic(true);
        wrong[0] = 0x00;
        assert!(Cartridge::parse(&wrong, "x").unwrap_err().contains("F0 43"));
    }

    /// Trailing bytes are ignored: editors append, and the cartridge is the
    /// first 4104 bytes.
    #[test]
    fn trailing_rubbish_does_not_stop_it() {
        let mut d = synthetic(true);
        d.extend_from_slice(&[0x00; 512]);
        assert!(Cartridge::parse(&d, "bank").is_ok());
    }

    /// **Against the real world.** Ten thousand cartridges off the internet,
    /// which is the only test that catches a format detail no document
    /// mentions. Ignored because the corpus is not in the repository; point it
    /// at a folder of `.syx` and it will walk the lot.
    ///
    ///   IVORY_SYX_CORPUS=~/Dropbox/Audio/Sysex cargo test -p ivory --bins \
    ///     the_whole_corpus -- --ignored --nocapture
    #[test]
    #[ignore = "needs a folder of cartridges"]
    fn the_whole_corpus_parses() {
        let Ok(root) = std::env::var("IVORY_SYX_CORPUS") else {
            eprintln!("IVORY_SYX_CORPUS not set; nothing was checked");
            return;
        };
        let mut walked = 0usize;
        let mut carts = 0usize;
        let mut bad_sum = 0usize;
        let mut named = 0usize;
        let mut stack = vec![std::path::PathBuf::from(root)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if !p
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("syx"))
                {
                    continue;
                }
                walked += 1;
                let Ok(c) = Cartridge::load(&p) else { continue };
                carts += 1;
                if !c.checksum_ok {
                    bad_sum += 1;
                }
                assert_eq!(c.voices.len(), 32, "{}", p.display());
                for v in &c.voices {
                    // Every field has to be inside the range the DX7 defines,
                    // or the synth will index a table out of bounds later.
                    assert!(v.algorithm < 32, "{}", p.display());
                    assert!(v.feedback < 8, "{}", p.display());
                    assert!(v.lfo_wave < 8, "{}", p.display());
                    assert!(v.transpose < 64, "{}", p.display());
                    for op in &v.ops {
                        assert!(op.coarse < 32, "{}", p.display());
                        assert!(op.detune < 16, "{}", p.display());
                        assert!(op.rate_scaling < 8, "{}", p.display());
                        assert!(op.vel_sens < 8, "{}", p.display());
                        assert!(op.left_curve < 4 && op.right_curve < 4, "{}", p.display());
                    }
                    if !v.display_name().is_empty() {
                        named += 1;
                    }
                }
            }
        }
        println!(
            "walked {walked} .syx, parsed {carts} cartridges ({bad_sum} with a bad checksum), \
             {named} named voices"
        );
        assert!(carts > 0, "no cartridges were found under IVORY_SYX_CORPUS");
    }
}
