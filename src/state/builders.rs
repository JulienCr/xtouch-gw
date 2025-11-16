//! Builder functions to construct state entries from raw MIDI data
//!
//! Converts raw MIDI bytes into structured MidiStateEntry objects.

use super::types::{
    compute_hash, MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin,
};
use crate::midi::{get_type_nibble, pb14_from_raw};
use std::time::{SystemTime, UNIX_EPOCH};

/// Constructs a MidiStateEntry from raw MIDI bytes
///
/// Returns None for unsupported message types or invalid data.
///
/// Handles:
/// - Note On/Off → value = velocity (0 = off)
/// - CC → value = 0..127
/// - PB → value = 0..16383 (14-bit)
/// - SysEx → value = Uint8Array (full payload)
pub fn build_entry_from_raw(raw: &[u8], port_id: &str) -> Option<MidiStateEntry> {
    if raw.is_empty() {
        return None;
    }

    let status = raw[0];
    let d1 = raw.get(1).copied().unwrap_or(0);
    let d2 = raw.get(2).copied().unwrap_or(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // SysEx (0xF0)
    if status == 0xF0 {
        let payload = raw.to_vec();
        let hash = compute_hash(&payload);
        return Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: port_id.to_string(),
                status: MidiStatus::SysEx,
                channel: None,
                data1: None,
            },
            value: MidiValue::Binary(payload),
            ts: now,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: Some(hash),
        });
    }

    // Ignore other system messages (>= 0xF0)
    if status >= 0xF0 {
        return None;
    }

    let type_nibble = get_type_nibble(status);
    let channel = (status & 0x0F) + 1; // Internal 0-15 → external 1-16

    // Note On (0x9x) or Note Off (0x8x)
    if type_nibble == 0x9 || type_nibble == 0x8 {
        let velocity = if type_nibble == 0x8 { 0 } else { d2 as u16 };
        return Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: port_id.to_string(),
                status: MidiStatus::Note,
                channel: Some(channel),
                data1: Some(d1),
            },
            value: MidiValue::Number(velocity),
            ts: now,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        });
    }

    // Control Change (0xBx)
    if type_nibble == 0xB {
        return Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: port_id.to_string(),
                status: MidiStatus::CC,
                channel: Some(channel),
                data1: Some(d1),
            },
            value: MidiValue::Number(d2 as u16),
            ts: now,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        });
    }

    // Pitch Bend (0xEx)
    if type_nibble == 0xE {
        let value14 = pb14_from_raw(d1, d2);
        return Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: port_id.to_string(),
                status: MidiStatus::PB,
                channel: Some(channel),
                data1: Some(0), // PB doesn't use data1
            },
            value: MidiValue::Number(value14),
            ts: now,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_note_on() {
        let raw = [0x90, 0x3C, 0x7F]; // Note On, Ch1, Middle C, velocity 127
        let entry = build_entry_from_raw(&raw, "test").unwrap();
        assert_eq!(entry.addr.status, MidiStatus::Note);
        assert_eq!(entry.addr.channel, Some(1));
        assert_eq!(entry.addr.data1, Some(0x3C));
        assert_eq!(entry.value.as_number(), Some(127));
    }

    #[test]
    fn test_build_note_off() {
        let raw = [0x80, 0x3C, 0x00]; // Note Off, Ch1, Middle C
        let entry = build_entry_from_raw(&raw, "test").unwrap();
        assert_eq!(entry.addr.status, MidiStatus::Note);
        assert_eq!(entry.value.as_number(), Some(0));
    }

    #[test]
    fn test_build_cc() {
        let raw = [0xB0, 0x07, 0x64]; // CC, Ch1, CC7 (volume), value 100
        let entry = build_entry_from_raw(&raw, "test").unwrap();
        assert_eq!(entry.addr.status, MidiStatus::CC);
        assert_eq!(entry.addr.channel, Some(1));
        assert_eq!(entry.addr.data1, Some(0x07));
        assert_eq!(entry.value.as_number(), Some(100));
    }

    #[test]
    fn test_build_pb() {
        let raw = [0xE0, 0x00, 0x40]; // PB, Ch1, center position
        let entry = build_entry_from_raw(&raw, "test").unwrap();
        assert_eq!(entry.addr.status, MidiStatus::PB);
        assert_eq!(entry.addr.channel, Some(1));
        assert_eq!(entry.addr.data1, Some(0)); // PB always uses data1=0
        let value = entry.value.as_number().unwrap();
        assert!(value > 8000 && value < 8300); // Around center
    }

    #[test]
    fn test_build_sysex() {
        let raw = [0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7]; // Universal SysEx
        let entry = build_entry_from_raw(&raw, "test").unwrap();
        assert_eq!(entry.addr.status, MidiStatus::SysEx);
        assert!(entry.hash.is_some());
        assert_eq!(entry.value.as_binary().unwrap(), &raw);
    }

    #[test]
    fn test_build_invalid() {
        let raw = [0xFF]; // System Reset (not supported)
        let entry = build_entry_from_raw(&raw, "test");
        assert!(entry.is_none());
    }
}

