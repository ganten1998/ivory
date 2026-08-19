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

/// The bank a fresh install plays: sixteen electric pianos and sixteen jazz
/// guitars, written for this engine.
///
/// **Compiled in, and that is a licensing decision as much as a convenience.**
/// The factory ROMs are Yamaha's and the banks people trade are their authors';
/// this one was built from a study of what those patches DO rather than from
/// what any of them contains — see `docs/DX7-BANK-01.md` — and is the app's to
/// give away. None of its thirty-two voices is byte-identical to anything in
/// the forty-two thousand unique patches the study parsed.
///
/// Four kilobytes. There is no version of "ship the sounds separately" that is
/// worth a file the installer could fail to place.
const FACTORY_BYTES: &[u8] = include_bytes!("../../assets/Tangent-01-Tines-and-Boxes.syx");

/// What the built-in instrument plays before anybody loads a cartridge.
///
/// Parsed at every call rather than cached: it happens once at launch and once
/// more if somebody goes back to it, and a `OnceLock` for four kilobytes of
/// arithmetic is a lock to reason about for no reason.
///
/// **Cannot fail in practice**, and does not pretend it might: the bytes are in
/// the binary and `the_factory_bank_is_a_cartridge` parses them on every build.
/// A parse error here would mean a corrupted executable, and the honest answer
/// to that is the patch that is written in the source.
pub fn factory() -> Cartridge {
    Cartridge::parse(FACTORY_BYTES, FACTORY_NAME).unwrap_or_else(|_| {
        Cartridge::of(FACTORY_NAME, vec![Voice::default()])
    })
}

/// What the picker calls the bank that is compiled in.
pub const FACTORY_NAME: &str = "Tangent Bank 01 - Tines and Boxes";

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

    /// A bank of `voices`, padded to thirty-two with the default patch.
    ///
    /// **Padded rather than refused.** A user's own bank starts with one patch
    /// in it, and a format that only holds exactly thirty-two would mean
    /// inventing a "partial cartridge" concept that nothing else in the world
    /// understands.
    pub fn of(name: &str, voices: Vec<Voice>) -> Self {
        let mut voices = voices;
        voices.truncate(VOICES);
        while voices.len() < VOICES {
            voices.push(Voice::default());
        }
        Self {
            voices,
            name: name.to_owned(),
            checksum_ok: true,
        }
    }

    /// The 4104 bytes of a 32-voice bulk dump.
    ///
    /// **A real one.** What comes out of here loads into Dexed, into a TX802,
    /// and into a DX7 — which is the difference between an editor and a toy,
    /// and it costs nothing beyond writing the inverse of the parser that was
    /// already here.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut d = vec![0u8; BULK_LEN];
        d[..6].copy_from_slice(&[0xF0, 0x43, 0x00, 0x09, 0x20, 0x00]);
        for (i, v) in self.voices.iter().take(VOICES).enumerate() {
            d[6 + i * PACKED..6 + (i + 1) * PACKED].copy_from_slice(&v.pack());
        }
        // Seven-bit two's complement over the data, which is what the DX7
        // writes and what `parse` checks.
        let sum: u32 = d[6..6 + VOICES * PACKED]
            .iter()
            .map(|b| u32::from(*b))
            .sum();
        d[BULK_LEN - 2] = ((sum.wrapping_neg()) & 0x7f) as u8;
        d[BULK_LEN - 1] = 0xF7;
        d
    }

    /// Write it to disk, creating the folder if it is not there.
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(path, self.to_bytes()).map_err(|e| format!("{}: {e}", path.display()))
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

    /// **What is written reads back as itself**, and a real parser agrees it
    /// is a cartridge: the header, the length, the terminator and the checksum
    /// are all checked by `parse`, which is the same code that reads other
    /// people's banks.
    #[test]
    fn a_written_cartridge_reads_back_as_the_same_patches() {
        let mut a = Voice::default();
        a.set_name("MINE 1");
        a.algorithm = 17;
        a.feedback = 5;
        a.ops[0].output_level = 93;
        a.ops[3].coarse = 11;
        let mut b = Voice::default();
        b.set_name("MINE 2");
        b.transpose = 30;

        let cart = Cartridge::of("user", vec![a, b]);
        let bytes = cart.to_bytes();
        assert_eq!(bytes.len(), BULK_LEN);

        let back = Cartridge::parse(&bytes, "user").expect("what we wrote is a cartridge");
        assert!(back.checksum_ok, "the checksum we wrote does not check out");
        assert_eq!(back.voices[0], a);
        assert_eq!(back.voices[1], b);
        // And the rest is the default patch, not silence.
        assert_eq!(back.voices[31], Voice::default());
        assert_eq!(back.names()[0], "MINE 1");
    }

    /// **A patch built by hand survives the whole trip**: edited row by row,
    /// packed into a bank, written, read back by the same parser other
    /// people's cartridges go through, and still the patch that was made.
    ///
    /// This is the assertion the editor exists for. Every step of it has its
    /// own test; what this catches is the seam between them.
    #[test]
    fn a_hand_built_patch_survives_a_save_and_a_reload() {
        use super::super::edit;

        let mut v = Voice::default();
        v.set_name("HANDMADE");
        // Move something in every group, including both halves of a shared
        // packed byte and a choice with named values.
        edit::apply(&mut v, 0, 0, 71); // OP1 rate 1
        edit::apply(&mut v, 0, 19, 5); // OP1 rate scaling
        edit::apply(&mut v, 0, 13, 11); // OP1 detune, which shares its byte
        edit::apply(&mut v, 2, 8, 84); // OP3 output level
        edit::apply(&mut v, 5, 10, 1); // OP6 fixed frequency
        edit::apply(&mut v, edit::GLOBAL, 0, 21); // algorithm 22
        edit::apply(&mut v, edit::GLOBAL, 15, 4); // LFO wave: sine
        edit::apply(&mut v, edit::GLOBAL, 18, 30); // transpose

        let dir = std::env::temp_dir().join("tangent-patch-trip");
        let path = dir.join("my-patches.syx");
        let _ = std::fs::remove_file(&path);
        Cartridge::of("mine", vec![v])
            .save(&path)
            .expect("the bank was written");

        let back = Cartridge::load(&path).expect("and read back");
        assert!(back.checksum_ok);
        assert_eq!(back.voices[0], v, "the patch changed on the way to disk");
        assert_eq!(back.names()[0], "HANDMADE");
        // And the editor shows what was set, rather than what it started from.
        let shown = edit::to_edit(&back.voices[0], "");
        assert_eq!(shown.groups[0].params[0].value, 71);
        assert_eq!(shown.groups[0].params[19].value, 5);
        assert_eq!(shown.groups[0].params[13].value, 11);
        assert_eq!(shown.groups[edit::GLOBAL].params[0].value, 21);
        assert_eq!(shown.algorithm, 21, "the diagram followed the algorithm");
        let _ = std::fs::remove_file(&path);
    }

    /// More than a bank's worth is truncated rather than refused, and fewer is
    /// padded. Neither is an error a person can do anything about.
    #[test]
    fn a_bank_is_always_thirty_two_voices() {
        assert_eq!(Cartridge::of("x", Vec::new()).voices.len(), 32);
        assert_eq!(Cartridge::of("x", vec![Voice::default(); 40]).voices.len(), 32);
    }

    /// **The bank that ships is a bank.** It is compiled into the binary, so
    /// this is the only thing standing between a bad copy and an app that has
    /// no sound on first launch — and it runs on every build.
    #[test]
    fn the_factory_bank_is_a_cartridge() {
        let c = factory();
        assert_eq!(c.voices.len(), 32);
        assert!(c.checksum_ok, "the bank we ship fails its own checksum");
        assert_eq!(c.name, FACTORY_NAME);

        // Every voice is named, and none of them is the empty patch: a bank
        // half full of INIT VOICE is a bank that was truncated somewhere.
        let names = c.names();
        assert!(
            names.iter().all(|n| n != "(unnamed)" && !n.is_empty()),
            "a voice in the shipped bank has no name: {names:?}"
        );
        assert!(
            c.voices.iter().all(|v| *v != Voice::default()),
            "the shipped bank contains the default patch, so it is padded"
        );

        // And every one of them makes a sound: at least one carrier with a
        // level above nothing. A silent patch in a shipped bank is a patch
        // somebody selects and concludes the app is broken.
        for (i, v) in c.voices.iter().enumerate() {
            let dest = super::super::algorithms::DEST[usize::from(v.algorithm).min(31)];
            let audible = v
                .ops
                .iter()
                .zip(dest)
                .any(|(op, d)| d == super::super::algorithms::OUT && op.output_level > 0);
            assert!(audible, "voice {i} ({}) has no audible carrier", names[i]);
        }
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
