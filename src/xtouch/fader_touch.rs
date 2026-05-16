//! Per-channel fader touch tracker (MCU touch detection).
//!
//! The X-Touch in MCU mode sends Note On (channel 0, notes 104-112,
//! velocity 127) when a fader is touched and Note On velocity 0 (or
//! Note Off) on release. While a fader is touched the user is actively
//! pushing it, so we must:
//!   - NOT squelch incoming PitchBend on that channel (it's real user input)
//!   - NOT apply motor setpoints to that channel (anti motor-fight)
//!
//! MCU touch note → router channel mapping:
//!   Note 104 → fader 1 (channel 1)
//!   Note 105 → fader 2 (channel 2)
//!   ...
//!   Note 111 → fader 8 (channel 8)
//!   Note 112 → master fader (channel 9)
//!
//! So `channel = note - 103`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Faders 1..=8 + master (9).
const FADER_CHANNELS: usize = 9;

const TOUCH_NOTE_MIN: u8 = 104;
const TOUCH_NOTE_MAX: u8 = 112;

/// Safety timeout: if we miss a Note Off (firmware glitch, disconnect),
/// clear the touch state after this many ms so the channel doesn't stay
/// stuck "touched forever" and block Windows feedback.
const TOUCH_SAFETY_TIMEOUT_MS: u64 = 5_000;

/// Lock-free per-channel touch state tracker.
///
/// Cloneable: internal state is shared via `Arc`. Cheap to clone (one
/// `Arc` bump). Indexed by router channel (1..=9).
#[derive(Clone)]
pub struct FaderTouchTracker {
    start: Instant,
    /// Per-channel "touched-until" timestamp (ms since `start`).
    /// `0` means not touched. While held, set to `now + safety_timeout`.
    touched_until_ms: Arc<[AtomicU64; FADER_CHANNELS]>,
}

impl FaderTouchTracker {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            touched_until_ms: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
        }
    }

    fn current_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Inspect a raw MIDI message. If it is a touch note (Note On/Off on
    /// channel 0, note 104-112), update internal state and return the
    /// router channel (1..=9). Otherwise return None.
    ///
    /// Touch semantics:
    ///   - Note On (0x90), velocity > 0  → touched
    ///   - Note On (0x90), velocity == 0 → released (treated as Note Off)
    ///   - Note Off (0x80)               → released
    pub fn observe_raw(&self, raw: &[u8]) -> Option<u8> {
        if raw.len() < 3 {
            return None;
        }
        let status = raw[0];
        let note = raw[1];
        let velocity = raw[2];

        // MCU touch notes are always on channel 0
        let is_note_on = status == 0x90;
        let is_note_off = status == 0x80;
        if !is_note_on && !is_note_off {
            return None;
        }
        if !(TOUCH_NOTE_MIN..=TOUCH_NOTE_MAX).contains(&note) {
            return None;
        }

        let channel = note - (TOUCH_NOTE_MIN - 1); // 104 → 1, …, 112 → 9
        let idx = (channel - 1) as usize;

        let touched = is_note_on && velocity > 0;
        let new_value = if touched {
            self.current_ms() + TOUCH_SAFETY_TIMEOUT_MS
        } else {
            0
        };
        self.touched_until_ms[idx].store(new_value, Ordering::Relaxed);

        Some(channel)
    }

    /// True if the given router channel (1..=9) is currently touched.
    /// Returns false for out-of-range channels.
    pub fn is_touched(&self, channel: u8) -> bool {
        if !(1..=FADER_CHANNELS as u8).contains(&channel) {
            return false;
        }
        let idx = (channel - 1) as usize;
        let until = self.touched_until_ms[idx].load(Ordering::Relaxed);
        until > 0 && self.current_ms() < until
    }
}

impl Default for FaderTouchTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note_on(note: u8, velocity: u8) -> Vec<u8> {
        vec![0x90, note, velocity]
    }

    fn note_off(note: u8) -> Vec<u8> {
        vec![0x80, note, 0]
    }

    #[test]
    fn initial_state_is_untouched() {
        let t = FaderTouchTracker::new();
        for ch in 1..=9 {
            assert!(!t.is_touched(ch), "channel {} should be untouched", ch);
        }
    }

    #[test]
    fn note_on_velocity_127_marks_touched() {
        let t = FaderTouchTracker::new();
        let ch = t.observe_raw(&note_on(104, 127));
        assert_eq!(ch, Some(1));
        assert!(t.is_touched(1));
        assert!(!t.is_touched(2));
    }

    #[test]
    fn note_on_velocity_zero_releases() {
        let t = FaderTouchTracker::new();
        t.observe_raw(&note_on(105, 127));
        assert!(t.is_touched(2));
        t.observe_raw(&note_on(105, 0));
        assert!(!t.is_touched(2));
    }

    #[test]
    fn note_off_releases() {
        let t = FaderTouchTracker::new();
        t.observe_raw(&note_on(106, 127));
        assert!(t.is_touched(3));
        t.observe_raw(&note_off(106));
        assert!(!t.is_touched(3));
    }

    #[test]
    fn master_fader_maps_to_channel_9() {
        let t = FaderTouchTracker::new();
        let ch = t.observe_raw(&note_on(112, 127));
        assert_eq!(ch, Some(9));
        assert!(t.is_touched(9));
    }

    #[test]
    fn non_touch_messages_return_none() {
        let t = FaderTouchTracker::new();
        // Note On but outside touch note range
        assert_eq!(t.observe_raw(&note_on(60, 127)), None);
        // Touch note but wrong channel (channel 1, status 0x91)
        assert_eq!(t.observe_raw(&[0x91, 104, 127]), None);
        // PitchBend
        assert_eq!(t.observe_raw(&[0xE0, 0x00, 0x40]), None);
        // CC
        assert_eq!(t.observe_raw(&[0xB0, 0x10, 0x40]), None);
        // Truncated
        assert_eq!(t.observe_raw(&[0x90, 104]), None);
    }

    #[test]
    fn out_of_range_channel_is_never_touched() {
        let t = FaderTouchTracker::new();
        t.observe_raw(&note_on(104, 127));
        assert!(!t.is_touched(0));
        assert!(!t.is_touched(10));
        assert!(!t.is_touched(255));
    }

    #[test]
    fn channels_are_independent() {
        let t = FaderTouchTracker::new();
        t.observe_raw(&note_on(104, 127)); // fader 1
        t.observe_raw(&note_on(108, 127)); // fader 5
        assert!(t.is_touched(1));
        assert!(!t.is_touched(2));
        assert!(!t.is_touched(3));
        assert!(!t.is_touched(4));
        assert!(t.is_touched(5));
        assert!(!t.is_touched(6));

        t.observe_raw(&note_off(104));
        assert!(!t.is_touched(1));
        assert!(t.is_touched(5)); // independent
    }

    #[test]
    fn clone_shares_state() {
        let t = FaderTouchTracker::new();
        let t2 = t.clone();
        t.observe_raw(&note_on(107, 127));
        assert!(t2.is_touched(4));
        t2.observe_raw(&note_off(107));
        assert!(!t.is_touched(4));
    }
}
