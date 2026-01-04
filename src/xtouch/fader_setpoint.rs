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
    /// Epoch counter for anti-obsolescence (per-channel)
    epoch: u32,
    /// Page epoch when this setpoint was last updated
    /// Used to detect stale setpoints after page changes
    page_epoch: u64,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            desired14: 0,
            epoch: 0,
            page_epoch: 0,
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
    /// Current page epoch (updated via set_page_epoch)
    current_page_epoch: Arc<RwLock<u64>>,
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
            current_page_epoch: Arc::new(RwLock::new(0)),
        };

        (scheduler, apply_rx)
    }

    /// Update the page epoch (call this when page changes)
    ///
    /// BUG-009 FIX: This invalidates all existing setpoints by changing the
    /// reference epoch. Subsequent calls to `get_desired()` will return None
    /// for setpoints that were stored with a different page epoch.
    pub fn set_page_epoch(&self, epoch: u64) {
        let mut current = self.current_page_epoch.write().unwrap();
        *current = epoch;
        trace!("FaderSetpoint: page epoch updated to {}", epoch);
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

        // Get current page epoch for staleness tracking (BUG-009 FIX)
        let current_page_epoch = *self.current_page_epoch.read().unwrap();

        // Update state and get new epoch
        let epoch_snapshot = {
            let mut channels = self.channels.write().unwrap();
            let state = channels.entry(channel).or_default();
            state.desired14 = clamped;
            state.epoch += 1;
            state.page_epoch = current_page_epoch; // Track which page this setpoint belongs to
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

    /// Get the current desired value for a channel if it's still valid
    ///
    /// BUG-009 FIX: Returns None if the stored setpoint was created for a
    /// different page epoch, preventing stale values from being used during
    /// rapid page changes.
    pub fn get_desired(&self, channel: u8) -> Option<u16> {
        let current_page_epoch = *self.current_page_epoch.read().unwrap();
        let channels = self.channels.read().unwrap();
        channels.get(&channel).and_then(|state| {
            if state.page_epoch == current_page_epoch {
                Some(state.desired14)
            } else {
                trace!(
                    "FaderSetpoint get_desired SKIPPED (stale): ch={} stored_page_epoch={} current_page_epoch={}",
                    channel,
                    state.page_epoch,
                    current_page_epoch
                );
                None
            }
        })
    }

    /// Get the current epoch for a channel (for debugging)
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

    /// BUG-009: Test that page epoch changes invalidate stale setpoints
    #[tokio::test]
    async fn test_page_epoch_invalidates_stale_setpoints() {
        let (setpoint, _rx) = FaderSetpoint::new();

        // Initial page epoch is 0, schedule a setpoint
        setpoint.schedule(1, 5000, Some(1000));

        // Setpoint should be readable (same page epoch)
        assert_eq!(setpoint.get_desired(1), Some(5000));

        // Simulate page change by updating page epoch
        setpoint.set_page_epoch(1);

        // Setpoint should now be stale (different page epoch)
        assert_eq!(setpoint.get_desired(1), None);

        // Schedule new setpoint with updated page epoch
        setpoint.schedule(1, 8000, Some(1000));

        // New setpoint should be readable
        assert_eq!(setpoint.get_desired(1), Some(8000));
    }

    /// BUG-009: Test that rapid page changes don't leak old values
    #[tokio::test]
    async fn test_rapid_page_changes_no_stale_values() {
        let (setpoint, _rx) = FaderSetpoint::new();

        // Page 0: Set fader 1 to 1000
        setpoint.schedule(1, 1000, Some(1000));
        assert_eq!(setpoint.get_desired(1), Some(1000));

        // Rapid page changes: 0 -> 1 -> 2
        setpoint.set_page_epoch(1);
        setpoint.set_page_epoch(2);

        // Old value should be stale
        assert_eq!(setpoint.get_desired(1), None);

        // Page 2: Set fader 1 to 2000
        setpoint.schedule(1, 2000, Some(1000));
        assert_eq!(setpoint.get_desired(1), Some(2000));

        // Another page change
        setpoint.set_page_epoch(3);

        // Value from page 2 should be stale
        assert_eq!(setpoint.get_desired(1), None);
    }
}
