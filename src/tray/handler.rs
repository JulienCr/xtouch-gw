//! Tray message handler - Tokio task that manages tray updates
//!
//! Bridges the async Tokio runtime with the blocking Windows tray UI thread.
//! Subscribes to driver status callbacks and forwards updates via crossbeam channels.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use parking_lot::RwLock;
use tracing::{debug, info, trace, warn};

use super::{ActivityDirection, ActivityTracker, ConnectionStatus, TrayUpdate};

/// Handler that manages tray UI updates from the Tokio runtime
///
/// This runs as a Tokio task and:
/// - Subscribes to driver status callbacks
/// - Maintains a map of current driver statuses
/// - Polls activity tracker periodically for LED updates
/// - Forwards status changes to the tray UI via crossbeam channel
/// - Sends initial status snapshot when drivers register
/// - Rate limits updates to prevent spam
pub struct TrayMessageHandler {
    /// Sender for updates to the tray UI (crossbeam for cross-thread communication)
    tray_tx: crossbeam::channel::Sender<TrayUpdate>,

    /// Current status of all registered drivers (using parking_lot for sync access)
    driver_statuses: Arc<RwLock<HashMap<String, ConnectionStatus>>>,

    /// Activity tracker for polling driver activity (optional for Phase 5)
    activity_tracker: Option<Arc<ActivityTracker>>,

    /// Activity poll interval in milliseconds
    activity_poll_interval_ms: u64,

    /// Last update time per driver for rate limiting (driver_name -> timestamp)
    last_update_times: Arc<RwLock<HashMap<String, Instant>>>,

    /// Minimum time between status updates for the same driver (ms)
    rate_limit_ms: u64,
}

impl TrayMessageHandler {
    /// Create a new tray message handler
    pub fn new(
        tray_tx: crossbeam::channel::Sender<TrayUpdate>,
        activity_tracker: Option<Arc<ActivityTracker>>,
        activity_poll_interval_ms: u64,
    ) -> Self {
        Self {
            tray_tx,
            driver_statuses: Arc::new(RwLock::new(HashMap::new())),
            activity_tracker,
            activity_poll_interval_ms,
            last_update_times: Arc::new(RwLock::new(HashMap::new())),
            rate_limit_ms: 50, // Minimum 50ms between updates for same driver
        }
    }

    /// Subscribe to a driver's connection status updates
    ///
    /// Returns a callback that should be registered with the driver.
    /// When the driver's status changes, it will call this callback,
    /// which forwards the update to the tray UI with rate limiting.
    pub fn subscribe_driver(&self, driver_name: String) -> Arc<dyn Fn(ConnectionStatus) + Send + Sync> {
        let tray_tx = self.tray_tx.clone();
        let driver_statuses = Arc::clone(&self.driver_statuses);
        let last_update_times = Arc::clone(&self.last_update_times);
        let rate_limit_ms = self.rate_limit_ms;
        let name = driver_name.clone();

        Arc::new(move |status: ConnectionStatus| {
            trace!("TrayHandler: {} status changed to {:?}", name, status);

            // Check if status actually changed (always send if different from previous)
            let status_changed = {
                let statuses = driver_statuses.read();
                statuses.get(&name) != Some(&status)
            };

            // Check rate limit (but always allow status changes)
            let should_send = if status_changed {
                // Status changed - always send, update rate limit timer
                let mut times = last_update_times.write();
                times.insert(name.clone(), Instant::now());
                true
            } else {
                // Same status - apply rate limiting
                let mut times = last_update_times.write();
                let now = Instant::now();

                if let Some(last_time) = times.get(&name) {
                    let elapsed = now.duration_since(*last_time).as_millis() as u64;
                    if elapsed < rate_limit_ms {
                        debug!("Rate limiting duplicate status update for {} ({}ms elapsed)", name, elapsed);
                        false
                    } else {
                        times.insert(name.clone(), now);
                        true
                    }
                } else {
                    times.insert(name.clone(), now);
                    true
                }
            };

            // Update our internal tracking (parking_lot RwLock is synchronous)
            {
                let mut statuses = driver_statuses.write();
                statuses.insert(name.clone(), status.clone());
            }

            // Forward to tray UI if not rate limited (non-blocking)
            if should_send {
                let update = TrayUpdate::DriverStatus {
                    name: name.clone(),
                    status: status.clone(),
                };

                if let Err(e) = tray_tx.try_send(update) {
                    if matches!(e, crossbeam::channel::TrySendError::Disconnected(_)) {
                        warn!("Tray channel disconnected, cannot send status update for {}", name);
                    } else {
                        warn!("Failed to send status update to tray: {}", e);
                    }
                }
            }
        })
    }

    /// Get current status of all drivers
    pub fn get_all_statuses(&self) -> HashMap<String, ConnectionStatus> {
        self.driver_statuses.read().clone()
    }

    /// Send initial status for a driver (used when driver is first initialized)
    pub fn send_initial_status(&self, driver_name: String, status: ConnectionStatus) {
        trace!("TrayHandler: sending initial status for {}: {:?}", driver_name, status);

        // Update internal tracking
        {
            let mut statuses = self.driver_statuses.write();
            statuses.insert(driver_name.clone(), status.clone());
        }

        // Send to tray UI
        let update = TrayUpdate::DriverStatus {
            name: driver_name,
            status,
        };

        if let Err(e) = self.tray_tx.try_send(update) {
            warn!("Failed to send initial status to tray: {}", e);
        }
    }

    /// Run the handler - polls activity and forwards updates to tray
    ///
    /// Continuously polls the ActivityTracker at the configured interval
    /// and sends activity snapshots to the tray UI for LED visualization.
    /// Handles channel disconnection gracefully.
    pub async fn run(self: Arc<Self>) {
        debug!("TrayMessageHandler started (poll interval: {}ms, rate limit: {}ms)",
              self.activity_poll_interval_ms, self.rate_limit_ms);

        if self.activity_tracker.is_none() {
            debug!("Activity tracking disabled, handler running in minimal mode");
            // Just keep alive without polling
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            }
        }

        let activity_tracker = self.activity_tracker.as_ref().unwrap();
        let poll_interval = tokio::time::Duration::from_millis(self.activity_poll_interval_ms);

        debug!("Activity polling enabled ({}ms interval)", self.activity_poll_interval_ms);

        let mut iteration_count = 0u64;

        loop {
            tokio::time::sleep(poll_interval).await;
            iteration_count += 1;

            // Build activity snapshot for all registered drivers
            let driver_names: Vec<String> = {
                let statuses = self.driver_statuses.read();
                statuses.keys().cloned().collect()
            };

            if driver_names.is_empty() {
                if iteration_count % 100 == 0 {
                    debug!("No drivers registered yet (iteration {})", iteration_count);
                }
                continue;
            }

            let mut activities = HashMap::new();
            let mut active_count = 0;

            for driver_name in &driver_names {
                // Check inbound activity
                let inbound_active = activity_tracker.is_active(driver_name, ActivityDirection::Inbound);
                if inbound_active {
                    active_count += 1;
                }
                activities.insert((driver_name.clone(), ActivityDirection::Inbound), inbound_active);

                // Check outbound activity
                let outbound_active = activity_tracker.is_active(driver_name, ActivityDirection::Outbound);
                if outbound_active {
                    active_count += 1;
                }
                activities.insert((driver_name.clone(), ActivityDirection::Outbound), outbound_active);
            }

            // Send snapshot to tray UI
            let update = TrayUpdate::ActivitySnapshot { activities };
            if let Err(e) = self.tray_tx.try_send(update) {
                if matches!(e, crossbeam::channel::TrySendError::Disconnected(_)) {
                    warn!("Tray channel disconnected, stopping handler");
                    break;
                } else {
                    warn!("Failed to send activity snapshot to tray: {}", e);
                }
            }

            // Log periodic stats
            /*
            if iteration_count % 100 == 0 {
                debug!(
                    "TrayHandler stats: {} drivers, {} active directions (iteration {})",
                    driver_names.len(),
                    active_count,
                    iteration_count
                );
            }
            */
        }

        //info!("TrayMessageHandler stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscribe_driver() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let handler = TrayMessageHandler::new(tx, None, 100);

        let callback = handler.subscribe_driver("TestDriver".to_string());

        // Trigger callback
        callback(ConnectionStatus::Connected);

        // Should receive update
        let update = rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap();
        match update {
            TrayUpdate::DriverStatus { name, status } => {
                assert_eq!(name, "TestDriver");
                assert_eq!(status, ConnectionStatus::Connected);
            }
            _ => panic!("Expected DriverStatus update"),
        }
    }

    #[tokio::test]
    async fn test_multiple_drivers() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let handler = TrayMessageHandler::new(tx, None, 100);

        let cb1 = handler.subscribe_driver("Driver1".to_string());
        let cb2 = handler.subscribe_driver("Driver2".to_string());

        cb1(ConnectionStatus::Connected);
        cb2(ConnectionStatus::Disconnected);

        // Should have both in tracking
        let statuses = handler.get_all_statuses();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses.get("Driver1"), Some(&ConnectionStatus::Connected));
        assert_eq!(statuses.get("Driver2"), Some(&ConnectionStatus::Disconnected));

        // Should receive both updates
        let u1 = rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap();
        let u2 = rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap();

        assert!(matches!(u1, TrayUpdate::DriverStatus { .. }));
        assert!(matches!(u2, TrayUpdate::DriverStatus { .. }));
    }

    #[tokio::test]
    async fn test_send_initial_status() {
        let (tx, rx) = crossbeam::channel::unbounded();
        let handler = TrayMessageHandler::new(tx, None, 100);

        handler.send_initial_status("OBS".to_string(), ConnectionStatus::Connected);

        // Should receive update
        let update = rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap();
        match update {
            TrayUpdate::DriverStatus { name, status } => {
                assert_eq!(name, "OBS");
                assert_eq!(status, ConnectionStatus::Connected);
            }
            _ => panic!("Expected DriverStatus update"),
        }

        // Should be in tracking
        let statuses = handler.get_all_statuses();
        assert_eq!(statuses.get("OBS"), Some(&ConnectionStatus::Connected));
    }

    #[tokio::test]
    async fn test_status_update_overwrites() {
        let (tx, _rx) = crossbeam::channel::unbounded();
        let handler = TrayMessageHandler::new(tx, None, 100);

        let callback = handler.subscribe_driver("OBS".to_string());

        callback(ConnectionStatus::Connected);
        callback(ConnectionStatus::Disconnected);
        callback(ConnectionStatus::Reconnecting { attempt: 1 });

        // Should have latest status
        let statuses = handler.get_all_statuses();
        assert_eq!(
            statuses.get("OBS"),
            Some(&ConnectionStatus::Reconnecting { attempt: 1 })
        );
    }
}
