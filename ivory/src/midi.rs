//! MIDI input: midir callback thread -> mpsc channel (spec §10).
//!
//! - Auto-connect priority: "USB-MIDI" -> "Scarlett" OR ("USB" AND "MIDI") -> first.
//! - note_on vel>0 = on; note_off or note_on vel==0 = off; CC64 >= 64 = sustain down.
//! - Channels merged (status high nibble only). Everything else ignored.
//! - No reconnect logic (parity): if the port dies, events just stop.

use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    Sustain { down: bool },
}

/// Holds the open connection; dropping it closes the port.
pub struct MidiConnection {
    _conn: midir::MidiInputConnection<()>,
    pub port_name: String,
}

fn new_input(client_name: &str) -> Option<midir::MidiInput> {
    midir::MidiInput::new(client_name).ok()
}

/// Names of all available MIDI input ports.
pub fn list_port_names() -> Vec<String> {
    let Some(input) = new_input("ivory-scan") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

/// Print the `--list` output with the parity strings (spec §2.1).
pub fn print_port_list() {
    let names = list_port_names();
    println!("Available MIDI Input Ports:");
    if names.is_empty() {
        println!("  No MIDI input ports found!");
    } else {
        for (i, name) in names.iter().enumerate() {
            println!("  {i}: {name}");
        }
    }
}

/// Open the port with this exact name. The egui context is woken on every event
/// so repaints are event-driven rather than busy-looped (D-UI-3).
pub fn connect_by_name(
    name: &str,
    tx: mpsc::Sender<MidiEvent>,
    ctx: egui::Context,
) -> Result<MidiConnection, String> {
    let input = new_input("ivory").ok_or_else(|| "MIDI system unavailable".to_string())?;
    let port = input
        .ports()
        .into_iter()
        .find(|p| input.port_name(p).as_deref() == Ok(name))
        .ok_or_else(|| format!("no port named '{name}'"))?;
    let port_name = name.to_owned();
    let conn = input
        .connect(
            &port,
            "ivory-in",
            move |_stamp, message, _| {
                if let Some(event) = parse_message(message) {
                    let _ = tx.send(event);
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            },
            (),
        )
        .map_err(|e| e.to_string())?;
    Ok(MidiConnection {
        _conn: conn,
        port_name,
    })
}

/// Startup auto-connect (spec §10 priority chain). Returns None when there are
/// no ports or opening fails — the app runs without MIDI, silently.
pub fn auto_connect(tx: mpsc::Sender<MidiEvent>, ctx: egui::Context) -> Option<MidiConnection> {
    let names = list_port_names();
    let chosen = pick_auto_port(&names)?;
    connect_by_name(&chosen, tx, ctx).ok()
}

/// The parity priority chain, split out for testing.
pub fn pick_auto_port(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    if let Some(n) = names.iter().find(|n| n.contains("USB-MIDI")) {
        return Some(n.clone());
    }
    if let Some(n) = names
        .iter()
        .find(|n| n.contains("Scarlett") || (n.contains("USB") && n.contains("MIDI")))
    {
        return Some(n.clone());
    }
    names.first().cloned()
}

fn parse_message(message: &[u8]) -> Option<MidiEvent> {
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

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn auto_connect_priority_chain() {
        assert_eq!(pick_auto_port(&v(&[])), None);
        assert_eq!(
            pick_auto_port(&v(&["Foo", "USB-MIDI 1", "Scarlett 2i2"])),
            Some("USB-MIDI 1".into())
        );
        assert_eq!(
            pick_auto_port(&v(&["Foo", "Scarlett 2i2", "Bar USB MIDI"])),
            Some("Scarlett 2i2".into())
        );
        assert_eq!(
            pick_auto_port(&v(&["Foo", "Bar USB MIDI thing"])),
            Some("Bar USB MIDI thing".into())
        );
        assert_eq!(pick_auto_port(&v(&["Alpha", "Beta"])), Some("Alpha".into()));
    }

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
