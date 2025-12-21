//! Activity tracking for system tray LED visualization
//!
//! Tracks in/out message activity for each driver using a DashMap-based
//! lock-free data structure with timestamp tracking.

use dashmap::DashMap;
use std::time::Instant;
use tracing::trace;

use super::{ActivityDirection, TrayUpdate};

/// Activity tracker for monitoring message flow
///
/// Uses DashMap for lock-free concurrent access. Tracks the last activity
/// timestamp for each driver+direction combination.
pub struct ActivityTracker {
    /// Key: "{driver}:{direction}", Value: timestamp of last activity
    activity_map: DashMap<String, Instant>,

    /// How long LEDs should stay lit (milliseconds)
    led_duration_ms: u64,

    /// Optional channel for sending updates to tray UI
    tray_tx: Option<crossbeam::channel::Sender<TrayUpdate>>,
}

impl ActivityTracker {
    /// Create a new activity tracker
    ///
    /// # Arguments
    /// * `led_duration_ms` - How long to keep LEDs active after last message
    /// * `tray_tx` - Optional channel for sending tray updates
    pub fn new(
        led_duration_ms: u64,
        tray_tx: Option<crossbeam::channel::Sender<TrayUpdate>>,
    ) -> Self {
        Self {
            activity_map: DashMap::new(),
            led_duration_ms,
            tray_tx,
        }
    }

    /// Record activity for a driver
    ///
    /// This is called on every message send/receive. Uses non-blocking
    /// try_send to avoid impacting MIDI latency if the tray UI is slow.
    ///
    /// # Arguments
    /// * `driver` - Driver name (e.g., "obs", "xtouch", "qlc")
    /// * `direction` - Inbound or Outbound
    pub fn record(&self, driver: &str, direction: ActivityDirection) {
        let key = format!("{}:{:?}", driver, direction);
        self.activity_map.insert(key, Instant::now());

        trace!("Activity: {} {:?}", driver, direction);

        // Try to send to tray UI (non-blocking)
        if let Some(ref tx) = self.tray_tx {
            let _ = tx.try_send(TrayUpdate::Activity {
                driver: driver.to_string(),
                direction,
            });
        }
    }

    /// Check if a driver+direction is currently active
    ///
    /// Returns true if activity was recorded within the LED duration window.
    ///
    /// # Arguments
    /// * `driver` - Driver name
    /// * `direction` - Inbound or Outbound
    pub fn is_active(&self, driver: &str, direction: ActivityDirection) -> bool {
        let key = format!("{}:{:?}", driver, direction);

        self.activity_map
            .get(&key)
            .map(|entry| {
                let elapsed_ms = entry.value().elapsed().as_millis() as u64;
                elapsed_ms < self.led_duration_ms
            })
            .unwrap_or(false)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_activity_tracking() {
        let tracker = ActivityTracker::new(100, None);

        // Record activity
        tracker.record("test_driver", ActivityDirection::Inbound);

        // Should be active immediately
        assert!(tracker.is_active("test_driver", ActivityDirection::Inbound));

        // Should not be active for outbound
        assert!(!tracker.is_active("test_driver", ActivityDirection::Outbound));

        // Wait for expiration
        sleep(Duration::from_millis(150));

        // Should no longer be active
        assert!(!tracker.is_active("test_driver", ActivityDirection::Inbound));
    }
}
