//! Fader setpoint scheduler - Epoch-based debounced motorized fader updates
//!
//! This module implements the TypeScript reference's epoch-based anti-obsolescence system.
//! Each channel spawns independent async tasks that send apply commands via a channel.
//!
//! ## Key Features:
//! - **Epoch-based cancellation**: New setpoints invalidate old pending tasks
//! - **Per-channel async tasks**: Independent tokio spawns per fader
//! - **Debounced application**: 90ms delay (0ms for extremes)
//! - **Channel-based application**: Sends commands to main loop for execution

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::trace;

/// Command to apply a fader setpoint
#[derive(Debug, Clone)]
pub struct ApplySetpointCmd {
    pub channel: u8,
    pub value14: u16,
    pub epoch: u32,
}

/// State for a single fader channel
struct ChannelState {
    /// Desired 14-bit position (source of truth)
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

/// Fader setpoint scheduler with epoch-based anti-obsolescence
#[derive(Clone)]
pub struct FaderSetpoint {
    /// Per-channel state (keyed by channel 1-9)
    channels: Arc<RwLock<HashMap<u8, ChannelState>>>,
    /// Channel to send apply commands
    apply_tx: mpsc::UnboundedSender<ApplySetpointCmd>,
}

impl FaderSetpoint {
    /// Create a new fader setpoint scheduler
    ///
    /// Returns the scheduler and a receiver for apply commands
    pub fn new() -> (Self, mpsc::UnboundedReceiver<ApplySetpointCmd>) {
        let (apply_tx, apply_rx) = mpsc::unbounded_channel();

        let scheduler = Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            apply_tx,
        };

        (scheduler, apply_rx)
    }

    /// Schedule a fader setpoint update
    ///
    /// Spawns an async task that sends an apply command after debounce delay.
    /// If a new setpoint arrives before the task executes, the old task's
    /// command will be ignored due to epoch mismatch.
    ///
    /// # Arguments
    ///
    /// * `channel` - MIDI channel (1-9, where 9 is master fader)
    /// * `value14` - 14-bit value (0-16383)
    /// * `delay_ms` - Optional delay override (default 90ms, 0ms for extremes)
    pub fn schedule(&self, channel: u8, value14: u16, delay_ms: Option<u64>) {
        if !(1..=9).contains(&channel) {
            return;
        }

        let clamped = value14.min(16383);

        // Update state and get new epoch
        let epoch_snapshot = {
            let mut channels = self.channels.write().unwrap();
            let state = channels.entry(channel).or_default();
            state.desired14 = clamped;
            state.epoch += 1;
            state.epoch
        };

        // Determine delay: 0ms for extremes (0 or 16383), otherwise 90ms
        let is_extreme = clamped == 0 || clamped == 16383;
        let eff_delay = delay_ms.unwrap_or(if is_extreme { 0 } else { 90 });

        trace!(
            "FaderSetpoint schedule: ch={} value={} delay={}ms epoch={}",
            channel,
            clamped,
            eff_delay,
            epoch_snapshot
        );

        // Spawn async task to send apply command after delay
        let channels_clone = self.channels.clone();
        let apply_tx = self.apply_tx.clone();

        tokio::spawn(async move {
            // Debounce delay
            if eff_delay > 0 {
                tokio::time::sleep(Duration::from_millis(eff_delay)).await;
            }

            // Check if epoch still matches (not obsolete)
            let should_apply = {
                let channels_read = channels_clone.read().unwrap();
                channels_read
                    .get(&channel)
                    .map(|state| state.epoch == epoch_snapshot)
                    .unwrap_or(false)
            };

            if should_apply {
                // Send apply command
                let cmd = ApplySetpointCmd {
                    channel,
                    value14: clamped,
                    epoch: epoch_snapshot,
                };

                let _ = apply_tx.send(cmd);
            } else {
                trace!(
                    "FaderSetpoint apply SKIPPED (obsolete): ch={} epoch={}",
                    channel,
                    epoch_snapshot
                );
            }
        });
    }

    /// Get the current desired value for a channel (for debugging/inspection)
    pub fn get_desired(&self, channel: u8) -> Option<u16> {
        let channels = self.channels.read().unwrap();
        channels.get(&channel).map(|state| state.desired14)
    }

    /// Get the current epoch for a channel (for testing)
    #[cfg(test)]
    pub fn get_epoch(&self, channel: u8) -> Option<u32> {
        let channels = self.channels.read().unwrap();
        channels.get(&channel).map(|state| state.epoch)
    }

    /// Check if an epoch is still current
    pub fn is_epoch_current(&self, channel: u8, epoch: u32) -> bool {
        let channels = self.channels.read().unwrap();
        channels
            .get(&channel)
            .map(|state| state.epoch == epoch)
            .unwrap_or(false)
    }
}

impl Default for FaderSetpoint {
    fn default() -> Self {
        Self::new().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_epoch_increments() {
        let (setpoint, _rx) = FaderSetpoint::new();

        setpoint.schedule(1, 100, Some(1000)); // Long delay
        let epoch1 = setpoint.get_epoch(1).unwrap();

        setpoint.schedule(1, 200, Some(1000)); // New value
        let epoch2 = setpoint.get_epoch(1).unwrap();

        assert_eq!(epoch2, epoch1 + 1);
    }

    #[tokio::test]
    async fn test_rapid_movements_only_apply_last() {
        let (setpoint, mut rx) = FaderSetpoint::new();

        // Simulate rapid A→B→C movements
        setpoint.schedule(1, 100, Some(50)); // A
        setpoint.schedule(1, 200, Some(50)); // B
        setpoint.schedule(1, 300, Some(50)); // C (final)

        // Collect all apply commands
        let mut commands = vec![];
        tokio::time::sleep(Duration::from_millis(150)).await;

        while let Ok(cmd) = rx.try_recv() {
            commands.push(cmd);
        }

        // Should only have one command (C)
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].value14, 300);
    }
}
