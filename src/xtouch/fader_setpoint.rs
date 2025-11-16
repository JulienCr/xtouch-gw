//! Fader setpoint scheduler - Simplified debounced motorized fader updates
//!
//! This module provides a simplified setpoint control for motorized faders.
//! Instead of complex async spawning, it uses a straightforward epoch-based
//! approach where the caller is responsible for the debouncing logic.
//!
//! ## Key Features:
//! - **Epoch-based tracking**: Prevents stale updates
//! - **Per-channel state**: Independent tracking for each fader
//! - **Simple API**: No complex async spawning

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// State for a single fader channel
struct ChannelState {
    /// Desired 14-bit position (0-16383)
    desired14: u16,
    /// Epoch counter for anti-obsolescence
    epoch: u32,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            desired14: 0,
            epoch: 0,
        }
    }
}

/// Fader setpoint scheduler
pub struct FaderSetpoint {
    /// Per-channel state (keyed by channel 1-9)
    channels: Arc<RwLock<HashMap<u8, ChannelState>>>,
}

impl FaderSetpoint {
    /// Create a new fader setpoint scheduler
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Schedule a fader setpoint update
    ///
    /// Returns the epoch number for this update. Callers should pass this
    /// epoch to `should_apply()` before actually sending to the hardware.
    ///
    /// # Arguments
    ///
    /// * `channel` - MIDI channel (1-9, where 9 is master fader)
    /// * `value14` - 14-bit value (0-16383)
    pub fn schedule(&self, channel: u8, value14: u16) -> Option<u32> {
        if !(1..=9).contains(&channel) {
            return None;
        }

        let clamped = value14.min(16383);

        let mut channels = self.channels.write().unwrap();
        let state = channels.entry(channel).or_default();
        state.desired14 = clamped;
        state.epoch += 1;
        Some(state.epoch)
    }

    /// Check if an epoch is still current and should be applied
    ///
    /// Returns Some(value) if the epoch is current, None otherwise.
    pub fn should_apply(&self, channel: u8, epoch: u32) -> Option<u16> {
        let channels = self.channels.read().unwrap();
        channels
            .get(&channel)
            .filter(|state| state.epoch == epoch)
            .map(|state| state.desired14)
    }

    /// Get the current desired value for a channel
    pub fn get_desired(&self, channel: u8) -> Option<u16> {
        let channels = self.channels.read().unwrap();
        channels.get(&channel).map(|state| state.desired14)
    }
}

impl Default for FaderSetpoint {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_and_check() {
        let setpoint = FaderSetpoint::new();

        // Schedule a value
        let epoch1 = setpoint.schedule(1, 100).unwrap();
        assert_eq!(setpoint.should_apply(1, epoch1), Some(100));

        // Schedule a new value (epoch changes)
        let epoch2 = setpoint.schedule(1, 200).unwrap();
        assert_ne!(epoch1, epoch2);

        // Old epoch should not apply
        assert_eq!(setpoint.should_apply(1, epoch1), None);

        // New epoch should apply
        assert_eq!(setpoint.should_apply(1, epoch2), Some(200));
    }

    #[test]
    fn test_clamping() {
        let setpoint = FaderSetpoint::new();

        // Value above max should be clamped
        let epoch = setpoint.schedule(1, 20000).unwrap();
        assert_eq!(setpoint.should_apply(1, epoch), Some(16383));
    }

    #[test]
    fn test_invalid_channel() {
        let setpoint = FaderSetpoint::new();

        // Invalid channels should return None
        assert_eq!(setpoint.schedule(0, 100), None);
        assert_eq!(setpoint.schedule(10, 100), None);
    }
}
