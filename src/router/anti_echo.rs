//! Anti-echo and Last-Write-Wins suppression logic
//!
//! Prevents feedback loops by tracking:
//! - Shadow state of sent values with time windows
//! - User action timestamps for Last-Write-Wins

use crate::state::{MidiStateEntry, MidiStatus};
use std::collections::HashMap;
use tracing::trace;

/// Generate a consistent shadow key for MIDI state tracking
/// Format: "{status_lowercase}|{channel}|{data1}"
fn make_shadow_key(status: MidiStatus, channel: u8, data1: u8) -> String {
    let status_str = match status {
        MidiStatus::Note => "note",
        MidiStatus::CC => "cc",
        MidiStatus::PB => "pb",
        MidiStatus::SysEx => "sysex",
    };
    format!("{}|{}|{}", status_str, channel, data1)
}

/// Generate shadow key from a MidiStateEntry
fn make_shadow_key_from_entry(entry: &MidiStateEntry) -> String {
    make_shadow_key(
        entry.addr.status,
        entry.addr.channel.unwrap_or(0),
        entry.addr.data1.unwrap_or(0),
    )
}

/// Anti-echo time windows (in milliseconds) per MIDI status type
pub(crate) const ANTI_ECHO_WINDOWS: &[(MidiStatus, u64)] = &[
    (MidiStatus::PB, 250),   // Pitch Bend: motors need time to settle
    (MidiStatus::CC, 100),   // Control Change: encoders can generate rapid changes
    (MidiStatus::Note, 10),  // Note On/Off: buttons are discrete events
    (MidiStatus::SysEx, 60), // SysEx: fallback for other messages
];

/// Last-Write-Wins grace periods (in milliseconds)
const LWW_GRACE_PERIOD_PB: u64 = 300;
const LWW_GRACE_PERIOD_CC: u64 = 50;

/// Shadow state entry (value + timestamp)
#[derive(Debug, Clone)]
pub struct ShadowEntry {
    pub value: u16,
    pub ts: u64,
}

impl ShadowEntry {
    pub fn new(value: u16) -> Self {
        Self {
            value,
            ts: super::Router::now_ms(),
        }
    }
}

impl super::Router {
    /// Get anti-echo window for a MIDI status type
    pub(crate) fn get_anti_echo_window(status: MidiStatus) -> u64 {
        ANTI_ECHO_WINDOWS
            .iter()
            .find(|(s, _)| *s == status)
            .map(|(_, ms)| *ms)
            .unwrap_or(60)
    }

    /// Check if a value should be suppressed due to anti-echo
    pub(crate) fn should_suppress_anti_echo(&self, app_key: &str, entry: &MidiStateEntry) -> bool {
        let app_shadows = match self.app_shadows.try_read() {
            Ok(shadows) => shadows,
            Err(_) => return false,
        };

        let app_shadow = match app_shadows.get(app_key) {
            Some(shadow) => shadow,
            None => return false,
        };

        let key = make_shadow_key_from_entry(entry);

        if let Some(prev) = app_shadow.get(&key) {
            let value = entry.value.as_number().unwrap_or(0);
            let window = Self::get_anti_echo_window(entry.addr.status);
            let elapsed = Self::now_ms().saturating_sub(prev.ts);

            // Suppress if value matches and within time window
            if prev.value == value && elapsed < window {
                trace!(
                    "Anti-echo suppression: {} {}ms < {}ms",
                    entry.addr.status,
                    elapsed,
                    window
                );
                return true;
            }
        }

        false
    }

    /// Update shadow state after sending to app
    pub(crate) fn update_app_shadow(&self, app_key: &str, entry: &MidiStateEntry) {
        let key = make_shadow_key_from_entry(entry);

        let value = entry.value.as_number().unwrap_or(0);
        let shadow_entry = ShadowEntry::new(value);

        let mut app_shadows = self.app_shadows.write().unwrap();
        let app_shadow = app_shadows
            .entry(app_key.to_string())
            .or_insert_with(HashMap::new);
        app_shadow.insert(key, shadow_entry);
    }

    /// Clear X-Touch shadow state (allows re-emission during refresh)
    pub(crate) fn clear_xtouch_shadow(&self) {
        // X-Touch shadow is per-app, clear all
        if let Ok(mut shadows) = self.app_shadows.write() {
            shadows.clear();
        }
    }

    /// Check Last-Write-Wins: should suppress feedback if user action was recent
    pub(crate) fn should_suppress_lww(&self, entry: &MidiStateEntry) -> bool {
        let key = make_shadow_key_from_entry(entry);

        // Use blocking read to avoid silent failures
        // This is acceptable since reads are fast and we need correctness
        let last_user_ts = self
            .last_user_action_ts
            .read()
            .map(|ts_map| ts_map.get(&key).copied().unwrap_or(0))
            .unwrap_or(0);

        let grace_period = match entry.addr.status {
            MidiStatus::PB => LWW_GRACE_PERIOD_PB,
            MidiStatus::CC => LWW_GRACE_PERIOD_CC,
            _ => 0,
        };

        let elapsed = Self::now_ms().saturating_sub(last_user_ts);

        if grace_period > 0 && elapsed < grace_period {
            trace!(
                "LWW suppression: {} {}ms < {}ms",
                entry.addr.status,
                elapsed,
                grace_period
            );
            return true;
        }

        false
    }

    /// Mark a user action from X-Touch (for Last-Write-Wins)
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
            },
            0xB => {
                // Control Change
                let cc = raw.get(1).copied().unwrap_or(0);
                make_shadow_key(MidiStatus::CC, channel, cc)
            },
            0xE => {
                // Pitch Bend
                make_shadow_key(MidiStatus::PB, channel, 0)
            },
            _ => return,
        };

        let mut ts_map = self.last_user_action_ts.write().unwrap();
        ts_map.insert(key, Self::now_ms());
    }
}

