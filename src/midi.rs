//! MIDI utilities and message types
//! 
//! Provides MIDI message parsing, encoding, and value conversions.

use std::fmt;

/// MIDI message types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiMessage {
    /// Note Off: channel (0-15), note (0-127), velocity (0-127)
    NoteOff { channel: u8, note: u8, velocity: u8 },
    
    /// Note On: channel (0-15), note (0-127), velocity (0-127)
    NoteOn { channel: u8, note: u8, velocity: u8 },
    
    /// Polyphonic Key Pressure: channel (0-15), note (0-127), pressure (0-127)
    PolyPressure { channel: u8, note: u8, pressure: u8 },
    
    /// Control Change: channel (0-15), cc (0-127), value (0-127)
    ControlChange { channel: u8, cc: u8, value: u8 },
    
    /// Program Change: channel (0-15), program (0-127)
    ProgramChange { channel: u8, program: u8 },
    
    /// Channel Pressure: channel (0-15), pressure (0-127)
    ChannelPressure { channel: u8, pressure: u8 },
    
    /// Pitch Bend: channel (0-15), value (0-16383, 14-bit)
    PitchBend { channel: u8, value: u16 },
    
    /// System Exclusive: manufacturer_id, data bytes
    SysEx { data: Vec<u8> },
    
    /// MIDI Time Code Quarter Frame
    MidiTimeCode { data: u8 },
    
    /// Song Position Pointer
    SongPosition { position: u16 },
    
    /// Song Select
    SongSelect { song: u8 },
    
    /// Tune Request
    TuneRequest,
    
    /// Timing Clock
    TimingClock,
    
    /// Start
    Start,
    
    /// Continue
    Continue,
    
    /// Stop
    Stop,
    
    /// Active Sensing
    ActiveSensing,
    
    /// System Reset
    SystemReset,
}

impl MidiMessage {
    /// Parse a MIDI message from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }
        
        let status = data[0];
        
        // Handle running status (data byte first)
        if status < 0x80 {
            // This would require maintaining running status state
            // For now, we'll skip this case
            return None;
        }
        
        // Channel messages (0x80-0xEF)
        if status < 0xF0 {
            let message_type = status & 0xF0;
            let channel = status & 0x0F;
            
            match message_type {
                0x80 => {
                    // Note Off
                    if data.len() < 3 { return None; }
                    Some(MidiMessage::NoteOff {
                        channel,
                        note: data[1] & 0x7F,
                        velocity: data[2] & 0x7F,
                    })
                }
                0x90 => {
                    // Note On (velocity 0 = Note Off)
                    if data.len() < 3 { return None; }
                    let note = data[1] & 0x7F;
                    let velocity = data[2] & 0x7F;
                    
                    if velocity == 0 {
                        Some(MidiMessage::NoteOff { channel, note, velocity: 0 })
                    } else {
                        Some(MidiMessage::NoteOn { channel, note, velocity })
                    }
                }
                0xA0 => {
                    // Polyphonic Key Pressure
                    if data.len() < 3 { return None; }
                    Some(MidiMessage::PolyPressure {
                        channel,
                        note: data[1] & 0x7F,
                        pressure: data[2] & 0x7F,
                    })
                }
                0xB0 => {
                    // Control Change
                    if data.len() < 3 { return None; }
                    Some(MidiMessage::ControlChange {
                        channel,
                        cc: data[1] & 0x7F,
                        value: data[2] & 0x7F,
                    })
                }
                0xC0 => {
                    // Program Change
                    if data.len() < 2 { return None; }
                    Some(MidiMessage::ProgramChange {
                        channel,
                        program: data[1] & 0x7F,
                    })
                }
                0xD0 => {
                    // Channel Pressure
                    if data.len() < 2 { return None; }
                    Some(MidiMessage::ChannelPressure {
                        channel,
                        pressure: data[1] & 0x7F,
                    })
                }
                0xE0 => {
                    // Pitch Bend
                    if data.len() < 3 { return None; }
                    let lsb = (data[1] & 0x7F) as u16;
                    let msb = (data[2] & 0x7F) as u16;
                    let value = (msb << 7) | lsb;
                    Some(MidiMessage::PitchBend { channel, value })
                }
                _ => None,
            }
        } else {
            // System messages (0xF0-0xFF)
            match status {
                0xF0 => {
                    // System Exclusive - find the end (0xF7)
                    if let Some(end) = data.iter().position(|&b| b == 0xF7) {
                        let sysex_data = data[1..end].to_vec();
                        Some(MidiMessage::SysEx { data: sysex_data })
                    } else {
                        None
                    }
                }
                0xF1 => {
                    // MIDI Time Code Quarter Frame
                    if data.len() < 2 { return None; }
                    Some(MidiMessage::MidiTimeCode { data: data[1] })
                }
                0xF2 => {
                    // Song Position Pointer
                    if data.len() < 3 { return None; }
                    let lsb = (data[1] & 0x7F) as u16;
                    let msb = (data[2] & 0x7F) as u16;
                    Some(MidiMessage::SongPosition { position: (msb << 7) | lsb })
                }
                0xF3 => {
                    // Song Select
                    if data.len() < 2 { return None; }
                    Some(MidiMessage::SongSelect { song: data[1] & 0x7F })
                }
                0xF6 => Some(MidiMessage::TuneRequest),
                0xF8 => Some(MidiMessage::TimingClock),
                0xFA => Some(MidiMessage::Start),
                0xFB => Some(MidiMessage::Continue),
                0xFC => Some(MidiMessage::Stop),
                0xFE => Some(MidiMessage::ActiveSensing),
                0xFF => Some(MidiMessage::SystemReset),
                _ => None,
            }
        }
    }
    
    /// Encode the message to MIDI bytes
    pub fn encode(&self) -> Vec<u8> {
        match *self {
            MidiMessage::NoteOff { channel, note, velocity } => {
                vec![0x80 | (channel & 0x0F), note & 0x7F, velocity & 0x7F]
            }
            MidiMessage::NoteOn { channel, note, velocity } => {
                vec![0x90 | (channel & 0x0F), note & 0x7F, velocity & 0x7F]
            }
            MidiMessage::PolyPressure { channel, note, pressure } => {
                vec![0xA0 | (channel & 0x0F), note & 0x7F, pressure & 0x7F]
            }
            MidiMessage::ControlChange { channel, cc, value } => {
                vec![0xB0 | (channel & 0x0F), cc & 0x7F, value & 0x7F]
            }
            MidiMessage::ProgramChange { channel, program } => {
                vec![0xC0 | (channel & 0x0F), program & 0x7F]
            }
            MidiMessage::ChannelPressure { channel, pressure } => {
                vec![0xD0 | (channel & 0x0F), pressure & 0x7F]
            }
            MidiMessage::PitchBend { channel, value } => {
                let lsb = (value & 0x7F) as u8;
                let msb = ((value >> 7) & 0x7F) as u8;
                vec![0xE0 | (channel & 0x0F), lsb, msb]
            }
            MidiMessage::SysEx { ref data } => {
                let mut result = vec![0xF0];
                result.extend_from_slice(data);
                result.push(0xF7);
                result
            }
            MidiMessage::MidiTimeCode { data } => vec![0xF1, data],
            MidiMessage::SongPosition { position } => {
                vec![0xF2, (position & 0x7F) as u8, ((position >> 7) & 0x7F) as u8]
            }
            MidiMessage::SongSelect { song } => vec![0xF3, song & 0x7F],
            MidiMessage::TuneRequest => vec![0xF6],
            MidiMessage::TimingClock => vec![0xF8],
            MidiMessage::Start => vec![0xFA],
            MidiMessage::Continue => vec![0xFB],
            MidiMessage::Stop => vec![0xFC],
            MidiMessage::ActiveSensing => vec![0xFE],
            MidiMessage::SystemReset => vec![0xFF],
        }
    }
    
    /// Get the channel for channel messages (0-15), None for system messages
    pub fn channel(&self) -> Option<u8> {
        match *self {
            MidiMessage::NoteOff { channel, .. } |
            MidiMessage::NoteOn { channel, .. } |
            MidiMessage::PolyPressure { channel, .. } |
            MidiMessage::ControlChange { channel, .. } |
            MidiMessage::ProgramChange { channel, .. } |
            MidiMessage::ChannelPressure { channel, .. } |
            MidiMessage::PitchBend { channel, .. } => Some(channel),
            _ => None,
        }
    }
    
    /// Check if this is a channel message
    pub fn is_channel_message(&self) -> bool {
        self.channel().is_some()
    }
    
    /// Check if this is a system message
    pub fn is_system_message(&self) -> bool {
        !self.is_channel_message()
    }
}

impl fmt::Display for MidiMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            MidiMessage::NoteOff { channel, note, velocity } => {
                write!(f, "NoteOff ch:{} n:{} v:{}", channel + 1, note, velocity)
            }
            MidiMessage::NoteOn { channel, note, velocity } => {
                write!(f, "NoteOn ch:{} n:{} v:{}", channel + 1, note, velocity)
            }
            MidiMessage::PolyPressure { channel, note, pressure } => {
                write!(f, "PolyPressure ch:{} n:{} p:{}", channel + 1, note, pressure)
            }
            MidiMessage::ControlChange { channel, cc, value } => {
                write!(f, "CC ch:{} cc:{} v:{}", channel + 1, cc, value)
            }
            MidiMessage::ProgramChange { channel, program } => {
                write!(f, "ProgramChange ch:{} p:{}", channel + 1, program)
            }
            MidiMessage::ChannelPressure { channel, pressure } => {
                write!(f, "ChannelPressure ch:{} p:{}", channel + 1, pressure)
            }
            MidiMessage::PitchBend { channel, value } => {
                write!(f, "PitchBend ch:{} v:{}", channel + 1, value)
            }
            MidiMessage::SysEx { ref data } => {
                write!(f, "SysEx {} bytes", data.len())
            }
            _ => write!(f, "{:?}", self),
        }
    }
}

/// MIDI value conversion utilities
pub mod convert {
    /// Convert 14-bit value (0-16383) to 7-bit value (0-127)
    pub fn to_7bit(value_14bit: u16) -> u8 {
        ((value_14bit >> 7) & 0x7F) as u8
    }
    
    /// Convert 7-bit value (0-127) to 14-bit value (0-16383)
    pub fn to_14bit(value_7bit: u8) -> u16 {
        ((value_7bit as u16) << 7) | (value_7bit as u16)
    }
    
    /// Convert 14-bit value to percentage (0-100)
    pub fn to_percent_14bit(value: u16) -> f32 {
        (value as f32 * 100.0) / 16383.0
    }
    
    /// Convert 7-bit value to percentage (0-100)
    pub fn to_percent_7bit(value: u8) -> f32 {
        (value as f32 * 100.0) / 127.0
    }
    
    /// Convert percentage to 14-bit value
    pub fn from_percent_14bit(percent: f32) -> u16 {
        ((percent.clamp(0.0, 100.0) * 16383.0) / 100.0) as u16
    }
    
    /// Convert percentage to 7-bit value
    pub fn from_percent_7bit(percent: f32) -> u8 {
        ((percent.clamp(0.0, 100.0) * 127.0) / 100.0) as u8
    }
    
    /// Convert 14-bit value to 8-bit value (0-255) - for 8-bit CC mode
    pub fn to_8bit(value_14bit: u16) -> u8 {
        ((value_14bit >> 6) & 0xFF) as u8
    }
    
    /// Convert 8-bit value to 14-bit value
    pub fn from_8bit(value_8bit: u8) -> u16 {
        (value_8bit as u16) << 6
    }
}

/// Format MIDI bytes as hex string for debugging
pub fn format_hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format MIDI message for sniffer output
pub fn format_sniffer(timestamp_ms: u64, direction: &str, port: &str, data: &[u8]) -> String {
    let hex = format_hex(data);
    let message = MidiMessage::parse(data)
        .map(|m| format!(" => {}", m))
        .unwrap_or_default();
    
    format!("[{:08}ms] {} {} | {}{}", 
        timestamp_ms, direction, port, hex, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_note_on_parsing() {
        let data = vec![0x90, 60, 100]; // Note On, ch 1, Middle C, velocity 100
        let msg = MidiMessage::parse(&data).unwrap();
        
        assert_eq!(msg, MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        });
    }
    
    #[test]
    fn test_note_on_velocity_zero() {
        let data = vec![0x90, 60, 0]; // Note On with velocity 0 = Note Off
        let msg = MidiMessage::parse(&data).unwrap();
        
        assert_eq!(msg, MidiMessage::NoteOff {
            channel: 0,
            note: 60,
            velocity: 0,
        });
    }
    
    #[test]
    fn test_control_change() {
        let data = vec![0xB2, 7, 100]; // CC ch 3, volume, value 100
        let msg = MidiMessage::parse(&data).unwrap();
        
        assert_eq!(msg, MidiMessage::ControlChange {
            channel: 2,
            cc: 7,
            value: 100,
        });
    }
    
    #[test]
    fn test_pitch_bend() {
        let data = vec![0xE0, 0x00, 0x40]; // Pitch Bend ch 1, center (8192)
        let msg = MidiMessage::parse(&data).unwrap();
        
        assert_eq!(msg, MidiMessage::PitchBend {
            channel: 0,
            value: 8192,
        });
    }
    
    #[test]
    fn test_encode_note_on() {
        let msg = MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        };
        
        assert_eq!(msg.encode(), vec![0x90, 60, 100]);
    }
    
    #[test]
    fn test_14bit_to_7bit() {
        assert_eq!(convert::to_7bit(0), 0);
        assert_eq!(convert::to_7bit(8192), 64);
        assert_eq!(convert::to_7bit(16383), 127);
    }
    
    #[test]
    fn test_7bit_to_14bit() {
        assert_eq!(convert::to_14bit(0), 0);
        assert_eq!(convert::to_14bit(64), 8192);
        assert_eq!(convert::to_14bit(127), 16256);
    }
    
    #[test]
    fn test_percent_conversions() {
        assert_eq!(convert::to_percent_14bit(0) as u32, 0);
        assert_eq!(convert::to_percent_14bit(8192) as u32, 50);
        assert_eq!(convert::to_percent_14bit(16383) as u32, 100);
        
        assert_eq!(convert::from_percent_14bit(0.0), 0);
        assert_eq!(convert::from_percent_14bit(50.0), 8191);
        assert_eq!(convert::from_percent_14bit(100.0), 16383);
    }
}