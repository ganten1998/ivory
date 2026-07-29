use std::sync::mpsc;

#[derive(Debug, Clone)]
pub enum MidiEvent {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    SustainPedal { active: bool },
}

pub struct MidiThread {
    _connection: midir::MidiInputConnection<()>,
}

impl MidiThread {
    /// Connect to the first available MIDI port and spawn a listener.
    /// Returns the connection guard (keep alive) and a receiver for events.
    pub fn connect(tx: mpsc::Sender<MidiEvent>) -> Option<Self> {
        let input = midir::MidiInput::new("ivory").ok()?;
        let ports = input.ports();
        if ports.is_empty() {
            return None;
        }
        // Pick the first port
        let port = ports.into_iter().next()?;
        let conn = input
            .connect(
                &port,
                "ivory-in",
                move |_stamp, message, _| {
                    parse_and_send(message, &tx);
                },
                (),
            )
            .ok()?;
        Some(MidiThread { _connection: conn })
    }

    /// List available MIDI port names.
    pub fn list_ports() -> Vec<String> {
        if let Ok(input) = midir::MidiInput::new("ivory-scan") {
            input
                .ports()
                .iter()
                .filter_map(|p| input.port_name(p).ok())
                .collect()
        } else {
            Vec::new()
        }
    }
}

fn parse_and_send(message: &[u8], tx: &mpsc::Sender<MidiEvent>) {
    if message.len() < 2 {
        return;
    }
    let status = message[0] & 0xF0;
    let data1 = message[1];
    let data2 = if message.len() > 2 { message[2] } else { 0 };

    let event = match status {
        0x90 if data2 > 0 => MidiEvent::NoteOn { note: data1, velocity: data2 },
        0x90 | 0x80 => MidiEvent::NoteOff { note: data1 },
        0xB0 if data1 == 64 => MidiEvent::SustainPedal { active: data2 >= 64 },
        _ => return,
    };
    let _ = tx.send(event);
}
