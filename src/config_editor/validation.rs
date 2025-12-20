//! Configuration validation
//!
//! Provides validation functions for all config fields with specific rules
//! for MIDI values, ports, channels, etc.

/// Validate MIDI port name (must not be empty)
pub fn validate_midi_port(port: &str) -> Option<String> {
    if port.trim().is_empty() {
        Some("MIDI port cannot be empty".to_string())
    } else {
        None
    }
}

/// Validate OBS port (1-65535)
pub fn validate_obs_port(port: u16) -> Option<String> {
    if port == 0 {
        Some("Port must be 1-65535".to_string())
    } else {
        None
    }
}

/// Validate MIDI channel (1-16)
pub fn validate_midi_channel(channel: u8) -> Option<String> {
    if channel < 1 || channel > 16 {
        Some("MIDI channel must be 1-16".to_string())
    } else {
        None
    }
}

/// Validate CC number (0-127)
pub fn validate_cc_number(cc: u8) -> Option<String> {
    if cc > 127 {
        Some("CC number must be 0-127".to_string())
    } else {
        None
    }
}

/// Validate note number (0-127)
pub fn validate_note_number(note: u8) -> Option<String> {
    if note > 127 {
        Some("Note number must be 0-127".to_string())
    } else {
        None
    }
}

/// Validate LCD color (0-7 for X-Touch)
pub fn validate_lcd_color(color: u8) -> Option<String> {
    if color > 7 {
        Some("Color must be 0-7".to_string())
    } else {
        None
    }
}

/// Validate page name (must not be empty)
pub fn validate_page_name(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        Some("Page name cannot be empty".to_string())
    } else {
        None
    }
}

/// Validate app name (must not be empty)
pub fn validate_app_name(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        Some("App name cannot be empty".to_string())
    } else {
        None
    }
}

/// Validate hostname (basic check for non-empty)
pub fn validate_hostname(host: &str) -> Option<String> {
    if host.trim().is_empty() {
        Some("Hostname cannot be empty".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_midi_channel() {
        assert!(validate_midi_channel(1).is_none());
        assert!(validate_midi_channel(16).is_none());
        assert!(validate_midi_channel(0).is_some());
        assert!(validate_midi_channel(17).is_some());
    }

    #[test]
    fn test_validate_cc_number() {
        assert!(validate_cc_number(0).is_none());
        assert!(validate_cc_number(127).is_none());
        assert!(validate_cc_number(128).is_some());
    }

    #[test]
    fn test_validate_obs_port() {
        assert!(validate_obs_port(4455).is_none());
        assert!(validate_obs_port(1).is_none());
        assert!(validate_obs_port(65535).is_none());
        assert!(validate_obs_port(0).is_some());
    }

    #[test]
    fn test_validate_lcd_color() {
        assert!(validate_lcd_color(0).is_none());
        assert!(validate_lcd_color(7).is_none());
        assert!(validate_lcd_color(8).is_some());
    }
}
