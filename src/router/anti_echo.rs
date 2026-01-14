//! Anti-echo and Last-Write-Wins suppression logic
//!
//! Most anti-echo logic is now handled by the StateActor.
//! This module provides helper functions for parsing raw MIDI
//! into shadow keys for the state actor.

use crate::state::{make_shadow_key, MidiStatus};

impl super::Router {
    /// Mark a user action from X-Touch (for Last-Write-Wins)
    ///
    /// Parses the raw MIDI message and forwards to the state actor.
    pub fn mark_user_action(&self, raw: &[u8]) {
        if raw.is_empty() {
            return;
        }

        let status = raw[0];
        if status >= 0xF0 {
            return; // Skip system messages
        }

        let type_nibble = (status & 0xF0) >> 4;
        let channel = (status & 0x0F) + 1;

        let key = match type_nibble {
            0x9 | 0x8 => {
                // Note On/Off
                let note = raw.get(1).copied().unwrap_or(0);
                make_shadow_key(MidiStatus::Note, channel, note)
            }
            0xB => {
                // Control Change
                let cc = raw.get(1).copied().unwrap_or(0);
                make_shadow_key(MidiStatus::CC, channel, cc)
            }
            0xE => {
                // Pitch Bend
                make_shadow_key(MidiStatus::PB, channel, 0)
            }
            _ => return,
        };

        // Fire-and-forget: mark user action in state actor
        self.state_actor.mark_user_action(key, Self::now_ms());
    }
}
