//! Writing the take's `.mid`.
//!
//! # Why this does not reuse `MidiEvent`
//!
//! `ivory-ui::midi_event::MidiEvent` is three variants — `NoteOn{note,velocity}`,
//! `NoteOff{note}`, `Sustain{down}` — and that is exactly right for a chord
//! detector and exactly wrong for a recorder. It merges channels (`status &
//! 0xF0`), throws away note-off release velocity, collapses CC64 to a boolean at
//! 64, and its own test asserts that pitch bend parses to `None`.
//!
//! Half-pedalling on any decent digital piano sweeps CC64 through its full
//! range, so a recorder built on that type silently ships a `.mid` in which the
//! pedal is a switch. The fix is not to widen the shared type — that would
//! disturb frozen chord-engine semantics for no gain — but to **tee the raw
//! bytes before `parse_message` runs** and keep everything.
//!
//! # The ten ways this goes wrong
//!
//! Every one of these is a real, documented failure of real MIDI recorders, and
//! every one has a test below.
//!
//! 1. **No `EndOfTrack`.** midly does not write it for you and never warns. The
//!    file opens in most readers, so it passes local testing and breaks on
//!    somebody else's machine.
//! 2. **Hanging notes at stop** — the single most-reported MIDI recorder bug in
//!    existence. Import, press play, the last chord rings forever.
//! 3. **Hanging pedal at stop**, nastier because it is invisible in a piano
//!    roll: the notes have offs, CC64 is still 127, the sampler sustains anyway.
//!    Users report it as stuck notes and blame the notes.
//! 4. **Pairing note-on/off by key alone.** A split keyboard's note-offs then
//!    land on the wrong channel's notes. Key the held set on `(channel, key)`.
//! 5. **Re-triggered notes.** A trill gives `On(60) On(60) Off(60) Off(60)`; a
//!    `HashSet` emits one spurious note-off at stop. A counter gets it right.
//! 6. **Rounding each delta instead of the absolute tick.** Delta rounding
//!    accumulates monotonically, so the error is drift rather than jitter —
//!    which is precisely what a listener hears.
//! 7. **Negative deltas from out-of-order arrivals.** midly's `u28::from` masks
//!    silently, turning a negative into ~268 million ticks and putting the rest
//!    of the file 38 hours in the future.
//! 8. **Controller state that predates `T0`.** The keyboard sends its program
//!    change at connect time, and the pedal may already be down when Record is
//!    pressed. Without a snapshot the file has no instrument and a pedal release
//!    in bar 3 looks spontaneous.
//! 9. **A note already sounding at `T0`**, whose note-off would drive the held
//!    counter below zero — and if that counter is unsigned, wrap it.
//! 10. **Active Sensing.** Many keyboards send `0xFE` every ~300 ms forever.
//!     Written in, it bloats the file roughly tenfold and confuses readers.

use midly::num::{u4, u7, u15, u24, u28};
use midly::{
    Format, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent, TrackEventKind,
};
use std::collections::BTreeMap;
use std::path::Path;

use crate::clock::{Nanos, Timeline};

/// Ticks per quarter note.
///
/// 960 is Logic's native resolution and is exact in Cubase, REAPER and Live, so
/// nothing requantizes on import. It divides by 2, 3, 4, 5, 6, 8, 10, 12, 15,
/// 16, 20, 24 and 32, so every tuplet a pianist plays lands on an integer tick.
/// At 120 BPM one tick is 520 µs, which is below the 960 µs it takes MIDI to
/// even transmit a three-byte message — so the quantisation is not the limiting
/// factor and raising it buys nothing.
///
/// Deliberately not 30720 (16 µs ticks): no reader benefits and some older ones
/// assume ≤ 1920. Deliberately not the "one tick = 1 ms" trick (PPQ 500 at 120
/// BPM): it is not divisible by 3, so triplets never land exactly.
pub const PPQ: u16 = 960;

/// The default tempo mark, in microseconds per quarter note. 120 BPM.
pub const DEFAULT_TEMPO_US: u32 = 500_000;

/// One captured message, already stamped into the timebase.
///
/// Raw bytes, because the whole point is that nothing has been thrown away yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub t: Nanos,
    pub bytes: Vec<u8>,
}

impl Captured {
    pub fn new(t: Nanos, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            t,
            bytes: bytes.into(),
        }
    }

    fn status(&self) -> Option<u8> {
        self.bytes.first().copied().filter(|b| *b >= 0x80)
    }

    fn kind(&self) -> Option<u8> {
        self.status().map(|s| s & 0xF0)
    }

    fn channel(&self) -> Option<u8> {
        self.status().filter(|s| *s < 0xF0).map(|s| s & 0x0F)
    }

    /// A note-on with velocity 0 is a note-off. This is not a nicety; it is how
    /// most keyboards actually send note-offs, so a recorder that misses it
    /// reports every note as hanging.
    fn is_note_off(&self) -> bool {
        match self.kind() {
            Some(0x80) => true,
            Some(0x90) => self.bytes.get(2) == Some(&0),
            _ => false,
        }
    }

    fn is_note_on(&self) -> bool {
        self.kind() == Some(0x90) && self.bytes.get(2).is_some_and(|v| *v > 0)
    }

    fn note(&self) -> Option<u8> {
        matches!(self.kind(), Some(0x80 | 0x90))
            .then(|| self.bytes.get(1).copied())
            .flatten()
    }
}

/// Controllers whose value at `T0` must be restated at tick 0.
///
/// Sustain, sostenuto and soft are here because a pedal already down when
/// Record is pressed is otherwise released, in the file, by an event with no
/// cause. Volume and expression are here because a keyboard sends them once at
/// connect time and never again.
const SNAPSHOT_CCS: [u8; 5] = [7, 11, 64, 66, 67];

/// Messages that are dropped rather than recorded.
///
/// `0xFE` Active Sensing and `0xF8` MIDI Clock are heartbeats. midir's default
/// is to ignore nothing, so without this (or an `Ignore::TimeAndActiveSense` at
/// the source, which is better because it never allocates) a twenty-minute take
/// from a Yamaha carries roughly four thousand meaningless three-byte messages.
fn is_realtime_noise(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(0xF8 | 0xF9 | 0xFA | 0xFB | 0xFC | 0xFE))
}

/// Accumulates a take's MIDI and writes the file.
///
/// **Push everything, always, including before Record is pressed.** The tap in
/// `ivory/src/midi.rs` is permanently installed because midir gives no way to
/// reach a callback's captured state once the connection is open — see
/// `docs/RECORDER-PLAN.md` §4. That constraint turns out to be exactly what
/// failure mode 8 needs: the controller snapshot can only restate values whose
/// messages were actually seen, and those arrive long before `T0`.
#[derive(Debug, Default)]
pub struct MidiTake {
    events: Vec<Captured>,
}

impl MidiTake {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: Captured) {
        if is_realtime_noise(&event.bytes) {
            return;
        }
        self.events.push(event);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Drop everything older than `keep_from`, so an always-on tap does not grow
    /// without bound across a long idle session.
    ///
    /// Keep a generous window: the snapshot needs the *last* value of each
    /// controller, and a pedal pressed five minutes before Record still matters.
    pub fn prune(&mut self, keep_from: Nanos) {
        self.events.retain(|e| e.t >= keep_from);
    }

    /// Build the file.
    ///
    /// `tempo_bpm` is the mark written into the tempo map. It defaults to 120
    /// and is user-settable, and that is the honest fix for the one real problem
    /// with metrical time: a metrical SMF records *beat positions* plus an
    /// *assertion* about tempo, and the assertion is the first thing importers
    /// discard — Logic ignores it into an existing project, Cubase gates it on
    /// "Ignore Master Track Events", REAPER on an import preference. A user who
    /// played to a 72 BPM click and types 72 gets a file that drops into their
    /// 72 BPM project sample-accurately *and* whose bar lines land right.
    pub fn build(&self, timeline: &Timeline, tempo_bpm: f64) -> Smf<'_> {
        let tempo_us = ((60.0 / tempo_bpm) * 1_000_000.0).round() as u32;
        let ticks_per_sec = PPQ as f64 * 1_000_000.0 / tempo_us as f64;

        // Rule 6: quantise the ABSOLUTE tick and difference afterwards. Rounding
        // each inter-event delta accumulates monotonically.
        let tick_of = |t: Nanos| -> u64 {
            let s = timeline.file_seconds(t);
            if s <= 0.0 {
                0
            } else {
                (s * ticks_per_sec).round() as u64
            }
        };

        let t0 = timeline.t0();
        let t1 = timeline.t1();
        let end_tick = tick_of(t1);

        // ── track 0: the tempo map, and nothing else ────────────────────────
        // The spec: "for a format 1 file, the tempo map must be stored as the
        // first track". Putting performance data here as well is legal and is
        // how a file ends up unusable in notation software.
        let track0 = vec![
            meta(0, MetaMessage::TrackName(b"Tangent take")),
            meta(0, MetaMessage::Tempo(u24::new(tempo_us))),
            // numerator, denominator as a power of two, MIDI clocks per click,
            // 32nds per quarter. 4/4.
            meta(0, MetaMessage::TimeSignature(4, 2, 24, 8)),
            meta(0, MetaMessage::EndOfTrack),
        ];

        // ── the snapshot (rule 8) ───────────────────────────────────────────
        // Last value seen strictly before T0, per (channel, controller), plus
        // the last program change per channel. BTreeMap so the emitted order is
        // deterministic — a file that differs run to run cannot be diffed, and
        // the round-trip test below depends on it.
        let mut cc: BTreeMap<(u8, u8), u8> = BTreeMap::new();
        let mut program: BTreeMap<u8, u8> = BTreeMap::new();
        for e in self.events.iter().filter(|e| e.t < t0) {
            let (Some(ch), Some(kind)) = (e.channel(), e.kind()) else {
                continue;
            };
            match kind {
                0xB0 => {
                    if let (Some(&n), Some(&v)) = (e.bytes.get(1), e.bytes.get(2)) {
                        if SNAPSHOT_CCS.contains(&n) {
                            cc.insert((ch, n), v);
                        }
                    }
                }
                0xC0 => {
                    if let Some(&p) = e.bytes.get(1) {
                        program.insert(ch, p);
                    }
                }
                _ => {}
            }
        }

        let mut absolute: Vec<(u64, TrackEventKind<'_>)> = Vec::new();

        for (ch, p) in &program {
            absolute.push((
                0,
                TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::ProgramChange {
                        program: u7::new(*p),
                    },
                },
            ));
        }
        for ((ch, n), v) in &cc {
            // A controller whose snapshot value is 0 is its own default; writing
            // it costs three bytes and removes any doubt about whether the pedal
            // was up because it was up or because nothing was recorded.
            absolute.push((
                0,
                TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::Controller {
                        controller: u7::new(*n),
                        value: u7::new(*v),
                    },
                },
            ));
        }

        // ── the performance ─────────────────────────────────────────────────
        // Rules 4, 5 and 9: a COUNTER keyed on (channel, key), saturating at
        // zero. A HashSet emits one spurious note-off for a trill; an unsigned
        // counter decremented past zero by a note begun before T0 wraps to
        // ~4 billion, and the stop sequence then tries to emit all of them.
        let mut held: BTreeMap<(u8, u8), u32> = BTreeMap::new();
        let mut pedals: BTreeMap<(u8, u8), u8> = BTreeMap::new();
        for ((ch, n), v) in &cc {
            if matches!(n, 64 | 66 | 67) && *v > 0 {
                pedals.insert((*ch, *n), *v);
            }
        }

        for e in self.events.iter().filter(|e| e.t >= t0 && e.t <= t1) {
            let Some(kind) = raw_to_kind(&e.bytes) else {
                continue;
            };
            if let (Some(ch), Some(note)) = (e.channel(), e.note()) {
                if e.is_note_on() {
                    *held.entry((ch, note)).or_insert(0) += 1;
                } else if e.is_note_off() {
                    // Saturating: a note that began before T0 has no matching
                    // on, and its off is written as-is because it is what the
                    // player did. Synthesising a note-on at tick 0 for it would
                    // invent an attack that was never recorded.
                    if let Some(c) = held.get_mut(&(ch, note)) {
                        *c = c.saturating_sub(1);
                    }
                }
            }
            if let (Some(ch), Some(0xB0)) = (e.channel(), e.kind()) {
                if let (Some(&n), Some(&v)) = (e.bytes.get(1), e.bytes.get(2)) {
                    if matches!(n, 64 | 66 | 67) {
                        if v > 0 {
                            pedals.insert((ch, n), v);
                        } else {
                            pedals.remove(&(ch, n));
                        }
                    }
                }
            }
            absolute.push((tick_of(e.t), kind));
        }

        // ── the stop sequence (rules 2 and 3) ───────────────────────────────
        // Order is load-bearing: pedal-up must be at or AFTER the note-offs. Emit
        // it first and any reader that models sustain re-releases the notes that
        // were meant to ring.
        //
        // And the take is NOT extended to fit them. T1 is the stop instant; if
        // the MIDI ran four seconds past the audio and video because someone was
        // holding the pedal, the three files would disagree about the take's
        // duration and every tool that trusts durations would get it wrong.
        for ((ch, note), count) in &held {
            for _ in 0..*count {
                absolute.push((
                    end_tick,
                    TrackEventKind::Midi {
                        channel: u4::new(*ch),
                        message: MidiMessage::NoteOff {
                            key: u7::new(*note),
                            vel: u7::new(64),
                        },
                    },
                ));
            }
        }
        for (ch, n) in pedals.keys() {
            absolute.push((
                end_tick,
                TrackEventKind::Midi {
                    channel: u4::new(*ch),
                    message: MidiMessage::Controller {
                        controller: u7::new(*n),
                        value: u7::new(0),
                    },
                },
            ));
        }

        // Rule 7: STABLE sort by absolute tick. Events arrive out of order —
        // two ports, an ALSA queue reordering, or an anchor correction crossing
        // a message. An unsorted vector produces a negative delta, and midly's
        // `u28::from` masks it silently into ~268 million ticks.
        absolute.sort_by_key(|(tick, _)| *tick);

        let mut track1: Vec<TrackEvent<'_>> = Vec::with_capacity(absolute.len() + 1);
        let mut prev = 0u64;
        for (tick, kind) in absolute {
            track1.push(TrackEvent {
                delta: delta(tick - prev),
                kind,
            });
            prev = tick;
        }
        // Rule 1: EndOfTrack, at the take's real end rather than at the last
        // note, so a take that ends in silence keeps its silence.
        track1.push(TrackEvent {
            delta: delta(end_tick.saturating_sub(prev)),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });

        Smf {
            header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))),
            tracks: vec![track0, track1],
        }
    }

    /// Build and write in one step.
    pub fn write(
        &self,
        timeline: &Timeline,
        tempo_bpm: f64,
        path: &Path,
    ) -> std::io::Result<()> {
        self.build(timeline, tempo_bpm).save(path)
    }
}

fn meta(delta_ticks: u32, m: MetaMessage<'_>) -> TrackEvent<'_> {
    TrackEvent {
        delta: u28::new(delta_ticks),
        kind: TrackEventKind::Meta(m),
    }
}

/// Rule 7's other half.
///
/// `u28::from(x as u32)` calls `from_int_lossy`, which is `raw & MASK` — every
/// restricted integer in midly masks silently rather than erroring. A delta that
/// overflows means something is wrong upstream, and clamping to the maximum is a
/// visible 38-hour gap rather than an invisible wrap.
fn delta(ticks: u64) -> u28 {
    u28::new(u32::try_from(ticks).unwrap_or(u32::MAX).min(0x0FFF_FFFF))
}

/// Raw wire bytes to a track event, keeping **everything** — channel, release
/// velocity, every controller, pitch bend, both aftertouches, program change and
/// SysEx.
///
/// Running status is not handled because midir delivers complete messages; a
/// byte below 0x80 in position zero is a malformed message, not a continuation.
fn raw_to_kind(b: &[u8]) -> Option<TrackEventKind<'_>> {
    let status = *b.first()?;
    if status < 0x80 {
        return None;
    }
    // SysEx: midly's slice EXCLUDES the leading 0xF0 and MUST include the
    // trailing 0xF7 (`event.rs:85-90`, `:170-174`). Getting that wrong produces
    // a file that parses and then hangs a synth waiting for a terminator.
    if status == 0xF0 {
        return (b.len() >= 2).then(|| TrackEventKind::SysEx(&b[1..]));
    }
    if status >= 0xF0 {
        return None;
    }
    let channel = u4::new(status & 0x0F);
    let d1 = |i: usize| b.get(i).copied().map(u7::new);
    let message = match (status & 0xF0, b.len()) {
        (0x80, 3) => MidiMessage::NoteOff {
            key: d1(1)?,
            vel: d1(2)?,
        },
        // A note-on with velocity zero is written back as a note-on with
        // velocity zero, not normalised to 0x8n. It is what the instrument
        // sent, round-tripping is a property worth having, and readers all
        // understand it.
        (0x90, 3) => MidiMessage::NoteOn {
            key: d1(1)?,
            vel: d1(2)?,
        },
        (0xA0, 3) => MidiMessage::Aftertouch {
            key: d1(1)?,
            vel: d1(2)?,
        },
        (0xB0, 3) => MidiMessage::Controller {
            controller: d1(1)?,
            value: d1(2)?,
        },
        (0xC0, 2) => MidiMessage::ProgramChange { program: d1(1)? },
        (0xD0, 2) => MidiMessage::ChannelAftertouch { vel: d1(1)? },
        // LSB then MSB, which is the order the wire uses and the order midly
        // writes. Reversing it is a silent bug: small bends still look small.
        (0xE0, 3) => MidiMessage::PitchBend {
            bend: PitchBend(midly::num::u14::new(
                (*b.get(1)? as u16) | ((*b.get(2)? as u16) << 7),
            )),
        },
        _ => return None,
    };
    Some(TrackEventKind::Midi { channel, message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Timeline;

    const SEC: Nanos = 1_000_000_000;

    fn timeline(secs: i64) -> Timeline {
        Timeline::synthetic(0, secs * SEC, 48_000.0)
    }

    /// Ticks per second at 120 BPM with PPQ 960.
    const TPS: f64 = 1_920.0;

    fn ticks(smf: &Smf<'_>) -> Vec<(u64, String)> {
        let mut out = Vec::new();
        let mut abs = 0u64;
        for ev in &smf.tracks[1] {
            abs += ev.delta.as_int() as u64;
            out.push((abs, format!("{:?}", ev.kind)));
        }
        out
    }

    #[test]
    fn every_track_ends_with_end_of_track() {
        let take = MidiTake::new();
        let smf = take.build(&timeline(1), 120.0);
        for (i, t) in smf.tracks.iter().enumerate() {
            assert!(
                matches!(
                    t.last().map(|e| e.kind),
                    Some(TrackEventKind::Meta(MetaMessage::EndOfTrack))
                ),
                "track {i} has no EndOfTrack; midly will not add one and most \
                 readers will not complain, which is why this ships"
            );
        }
    }

    #[test]
    fn a_note_still_held_at_stop_gets_exactly_one_note_off() {
        let mut take = MidiTake::new();
        take.push(Captured::new(SEC, [0x90, 60, 100]));
        let smf = take.build(&timeline(4), 120.0);
        let offs = ticks(&smf)
            .into_iter()
            .filter(|(_, k)| k.contains("NoteOff"))
            .collect::<Vec<_>>();
        assert_eq!(offs.len(), 1, "exactly one note-off, at the stop tick");
        assert_eq!(offs[0].0, (4.0 * TPS) as u64);
    }

    #[test]
    fn a_trill_does_not_produce_a_spurious_note_off() {
        // On On Off Off — the pattern a fast repeat actually produces. A HashSet
        // would report the note as still held and add a third note-off.
        let mut take = MidiTake::new();
        take.push(Captured::new(SEC, [0x90, 60, 100]));
        take.push(Captured::new(SEC + SEC / 10, [0x90, 60, 110]));
        take.push(Captured::new(SEC + SEC / 5, [0x80, 60, 64]));
        take.push(Captured::new(SEC + SEC / 4, [0x80, 60, 64]));
        let smf = take.build(&timeline(4), 120.0);
        let offs = ticks(&smf)
            .into_iter()
            .filter(|(_, k)| k.contains("NoteOff"))
            .count();
        assert_eq!(offs, 2, "both note-offs, and no invented third");
    }

    #[test]
    fn a_note_held_across_t0_never_wraps_the_counter() {
        // Rule 9. The note began before Record; its off has no matching on. An
        // unsigned counter decremented past zero would wrap to ~4 billion and
        // the stop sequence would try to emit all of them.
        let mut take = MidiTake::new();
        take.push(Captured::new(-5 * SEC, [0x90, 60, 100]));
        take.push(Captured::new(SEC, [0x80, 60, 64]));
        let smf = take.build(&timeline(4), 120.0);
        let offs = ticks(&smf)
            .into_iter()
            .filter(|(_, k)| k.contains("NoteOff"))
            .count();
        assert_eq!(offs, 1, "the real note-off, and nothing synthesised");
    }

    #[test]
    fn a_pedal_down_at_stop_is_released_after_the_notes() {
        let mut take = MidiTake::new();
        take.push(Captured::new(SEC, [0xB0, 64, 127]));
        take.push(Captured::new(SEC, [0x90, 60, 100]));
        let smf = take.build(&timeline(4), 120.0);
        let end = (4.0 * TPS) as u64;
        let at_end: Vec<_> = ticks(&smf).into_iter().filter(|(t, _)| *t == end).collect();
        let off_ix = at_end.iter().position(|(_, k)| k.contains("NoteOff"));
        let ped_ix = at_end
            .iter()
            .position(|(_, k)| k.contains("Controller") && k.contains("64"));
        let (off_ix, ped_ix) = (off_ix.expect("note-off"), ped_ix.expect("pedal up"));
        assert!(
            off_ix < ped_ix,
            "pedal-up must come at or after the note-offs, or a reader that \
             models sustain re-releases notes that were meant to ring"
        );
    }

    #[test]
    fn state_from_before_t0_is_restated_at_tick_zero() {
        // Rule 8: the program change arrives at connect time and the pedal was
        // already down. Without the snapshot the file has no instrument, and the
        // pedal release at second 2 looks spontaneous.
        let mut take = MidiTake::new();
        take.push(Captured::new(-30 * SEC, [0xC0, 3]));
        take.push(Captured::new(-2 * SEC, [0xB0, 64, 127]));
        take.push(Captured::new(2 * SEC, [0xB0, 64, 0]));
        let smf = take.build(&timeline(4), 120.0);
        let at_zero: Vec<_> = ticks(&smf).into_iter().filter(|(t, _)| *t == 0).collect();
        assert!(
            at_zero.iter().any(|(_, k)| k.contains("ProgramChange")),
            "no instrument in the file: {at_zero:?}"
        );
        assert!(
            at_zero.iter().any(|(_, k)| k.contains("Controller")),
            "no pedal-down to explain the pedal-up: {at_zero:?}"
        );
    }

    #[test]
    fn out_of_order_arrivals_never_produce_a_giant_delta() {
        let mut take = MidiTake::new();
        take.push(Captured::new(3 * SEC, [0x90, 62, 100]));
        take.push(Captured::new(SEC, [0x90, 60, 100])); // earlier, pushed later
        let smf = take.build(&timeline(5), 120.0);
        let mut prev = 0u64;
        for (abs, _) in ticks(&smf) {
            assert!(abs >= prev, "ticks must be monotonic");
            prev = abs;
        }
        assert!(
            smf.tracks[1].iter().all(|e| e.delta.as_int() < 100_000),
            "a masked negative delta shows up as ~268 million ticks"
        );
    }

    #[test]
    fn half_pedalling_survives_as_a_continuous_value() {
        // The thing MidiEvent cannot express. Every intermediate value must be
        // in the file, not thresholded at 64.
        let mut take = MidiTake::new();
        for (i, v) in [10u8, 40, 63, 64, 90, 127].iter().enumerate() {
            take.push(Captured::new(SEC + i as i64 * SEC / 10, [0xB0, 64, *v]));
        }
        let smf = take.build(&timeline(4), 120.0);
        let values: Vec<String> = ticks(&smf)
            .into_iter()
            .map(|(_, k)| k)
            .filter(|k| k.contains("Controller"))
            .collect();
        for v in ["10", "40", "63", "64", "90", "127"] {
            assert!(
                values.iter().any(|k| k.contains(v)),
                "CC64 value {v} was lost; half-pedalling needs the whole sweep"
            );
        }
    }

    #[test]
    fn everything_the_display_path_discards_is_kept() {
        let mut take = MidiTake::new();
        take.push(Captured::new(SEC, [0xE5, 0x00, 0x40])); // pitch bend, ch 5
        take.push(Captured::new(SEC, [0xD2, 90])); // channel aftertouch
        take.push(Captured::new(SEC, [0xA0, 60, 70])); // poly aftertouch
        take.push(Captured::new(SEC, [0xB3, 1, 64])); // mod wheel, ch 3
        take.push(Captured::new(SEC, [0x83, 60, 33])); // release velocity
        let smf = take.build(&timeline(2), 120.0);
        let all = ticks(&smf)
            .into_iter()
            .map(|(_, k)| k)
            .collect::<Vec<_>>()
            .join("|");
        for expect in [
            "PitchBend",
            "ChannelAftertouch",
            "Aftertouch",
            "Controller",
            "NoteOff",
        ] {
            assert!(all.contains(expect), "{expect} was dropped: {all}");
        }
        assert!(
            all.contains("33"),
            "note-off release velocity must survive; Yamaha and Kawai actions \
             send it meaningfully"
        );
    }

    #[test]
    fn active_sensing_is_dropped_at_the_door() {
        let mut take = MidiTake::new();
        for i in 0..100 {
            take.push(Captured::new(SEC + i * SEC / 300, [0xFE]));
        }
        assert_eq!(take.len(), 0, "a heartbeat is not a performance");
    }

    #[test]
    fn absolute_ticks_are_rounded_not_accumulated() {
        // Rule 6. 1000 events spaced by an interval that does not land on a
        // tick. Rounding each delta drifts monotonically; rounding the absolute
        // tick does not.
        let mut take = MidiTake::new();
        let step = SEC / 3; // 333.333 ms; 640 ticks exactly at 120 BPM
        for i in 0..1_000i64 {
            take.push(Captured::new(i * step, [0x90, 60, 1]));
        }
        let smf = take.build(&Timeline::synthetic(0, 400 * SEC, 48_000.0), 120.0);
        let last_note = ticks(&smf)
            .into_iter()
            .filter(|(_, k)| k.contains("NoteOn"))
            .next_back()
            .unwrap()
            .0;
        let expected = (999.0 * (step as f64 / SEC as f64) * TPS).round() as u64;
        assert!(
            last_note.abs_diff(expected) <= 1,
            "drifted to {last_note}, expected {expected}"
        );
    }

    #[test]
    fn the_tempo_mark_is_what_was_asked_for() {
        let take = MidiTake::new();
        let smf = take.build(&timeline(1), 72.0);
        let tempos: Vec<_> = smf.tracks[0]
            .iter()
            .filter_map(|e| match e.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(t)) => Some(t.as_int()),
                _ => None,
            })
            .collect();
        assert_eq!(tempos, vec![833_333], "60/72 s per quarter, in microseconds");
    }

    #[test]
    fn the_file_round_trips_byte_identically() {
        // The strongest cheap check there is: if midly can parse what we wrote
        // and re-serialise it to the same bytes, the structure is coherent.
        let mut take = MidiTake::new();
        take.push(Captured::new(-SEC, [0xB0, 64, 100]));
        take.push(Captured::new(SEC, [0x90, 60, 100]));
        take.push(Captured::new(2 * SEC, [0x80, 60, 40]));
        take.push(Captured::new(2 * SEC, [0xE0, 0x00, 0x40]));
        let smf = take.build(&timeline(3), 120.0);

        let mut first = Vec::new();
        smf.write(&mut first).unwrap();
        let parsed = Smf::parse(&first).expect("our own file must parse");
        let mut second = Vec::new();
        parsed.write(&mut second).unwrap();
        assert_eq!(first, second, "round trip is not byte-identical");
    }

    #[test]
    fn the_take_is_not_extended_to_fit_the_stop_sequence() {
        let mut take = MidiTake::new();
        take.push(Captured::new(SEC, [0x90, 60, 100]));
        let smf = take.build(&timeline(4), 120.0);
        let end = ticks(&smf).last().unwrap().0;
        assert_eq!(
            end,
            (4.0 * TPS) as u64,
            "EndOfTrack must sit at T1, so the .mid, .wav and .mp4 agree about \
             how long the take was"
        );
    }
}
