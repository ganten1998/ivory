//! What a MIDI keyboard says, once the wire format is gone.
//!
//! Deliberately three variants and no more. Everything the app does with MIDI
//! is decided by `NoteState::apply`, and this is its whole alphabet: the parity
//! rules (spec §10) fold channels together, treat a note-on with velocity 0 as
//! a note-off, and read CC64 as the only controller that matters.
//!
//! It lives here rather than beside the `midir` connection because a VST3 build
//! has no `midir`: the host hands it note events directly, and it needs to say
//! the same three things about them. `parse_message` comes along because a
//! plugin still has to read raw bytes for anything the host passes through
//! verbatim, and because its rules ARE the parity rules.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    Sustain { down: bool },
}

/// One raw MIDI message. None for anything the app does not act on.
pub fn parse_message(message: &[u8]) -> Option<MidiEvent> {
    if message.len() < 3 {
        return None;
    }
    let status = message[0] & 0xF0;
    let data1 = message[1];
    let data2 = message[2];
    match status {
        0x90 if data2 > 0 => Some(MidiEvent::NoteOn {
            note: data1,
            velocity: data2,
        }),
        0x90 | 0x80 => Some(MidiEvent::NoteOff { note: data1 }),
        0xB0 if data1 == 64 => Some(MidiEvent::Sustain { down: data2 >= 64 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_parsing_semantics() {
        // note_on vel>0
        assert_eq!(
            parse_message(&[0x91, 60, 100]),
            Some(MidiEvent::NoteOn {
                note: 60,
                velocity: 100
            })
        );
        // note_on vel==0 == note_off; channel merged
        assert_eq!(
            parse_message(&[0x95, 60, 0]),
            Some(MidiEvent::NoteOff { note: 60 })
        );
        assert_eq!(
            parse_message(&[0x80, 61, 40]),
            Some(MidiEvent::NoteOff { note: 61 })
        );
        // sustain threshold at 64
        assert_eq!(
            parse_message(&[0xB0, 64, 64]),
            Some(MidiEvent::Sustain { down: true })
        );
        assert_eq!(
            parse_message(&[0xB2, 64, 63]),
            Some(MidiEvent::Sustain { down: false })
        );
        // other CCs / messages ignored
        assert_eq!(parse_message(&[0xB0, 1, 127]), None);
        assert_eq!(parse_message(&[0xE0, 0, 64]), None);
    }
}
