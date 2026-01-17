//! Router module - Core orchestration of MIDI events and page management
//!
//! The Router is the central orchestrator that manages:
//! - Page navigation and control mapping resolution
//! - MIDI state tracking per application
//! - Driver registration and execution
//! - Anti-echo windows and Last-Write-Wins logic
//! - Page refresh with state replay

mod anti_echo;
mod camera_target;
mod driver;
mod feedback;
mod indicators;
mod page;
mod refresh;
mod xtouch_input;

pub use camera_target::CameraTargetState;

#[cfg(test)]
mod tests;

use crate::config::AppConfig;
use crate::drivers::Driver;
use crate::state::persistence_actor::PersistenceActor;
use crate::state::{PersistenceActorHandle, StateActorHandle, DEFAULT_DEBOUNCE_MS};
use crate::xtouch::fader_setpoint::{ApplySetpointCmd, FaderSetpoint};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};

/// Main router orchestrating page navigation, state management, and driver execution
pub struct Router {
    /// Application configuration
    pub(crate) config: Arc<RwLock<AppConfig>>,
    /// Registered drivers by name
    pub(crate) drivers: Arc<RwLock<HashMap<String, Arc<dyn Driver>>>>,
    /// Active page index
    pub(crate) active_page_index: Arc<RwLock<usize>>,
    /// State actor handle for MIDI state management (per application)
    pub(crate) state_actor: StateActorHandle,
    /// Persistence actor handle for state snapshots
    pub(crate) persistence_actor: PersistenceActorHandle,
    /// Flag indicating display needs update after page change
    pub(crate) display_needs_update: Arc<tokio::sync::Mutex<bool>>,
    /// Fader setpoint scheduler (motor position tracking)
    pub(crate) fader_setpoint: Arc<FaderSetpoint>,
    /// Receiver for setpoint apply commands (stored for retrieval)
    pub(crate) setpoint_rx:
        Arc<tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<ApplySetpointCmd>>>>,
    /// Pending MIDI messages to send to X-Touch (e.g., from page refresh)
    pub(crate) pending_midi_messages: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    /// Activity tracker for tray UI LED visualization
    pub(crate) activity_tracker: Option<Arc<crate::tray::ActivityTracker>>,
    /// Page epoch counter - incremented on each page change to invalidate stale feedback
    /// BUG-006 FIX: Prevents race condition between page refresh and feedback processing
    pub(crate) page_epoch: Arc<AtomicU64>,
    /// Dynamic camera target state for Stream Deck integration
    pub(crate) camera_targets: Arc<CameraTargetState>,
}

impl Router {
    /// Create a new Router with initial configuration
    pub fn new(config: AppConfig) -> Self {
        Self::with_db_path(config, ".state/sled")
    }

    /// Create a new Router with a custom database path
    ///
    /// This is useful for testing to avoid database lock conflicts.
    pub fn with_db_path(config: AppConfig, db_path: &str) -> Self {
        let (fader_setpoint, setpoint_rx) = FaderSetpoint::new();

        // Spawn persistence actor for debounced state snapshots
        let persistence_actor = PersistenceActor::spawn(db_path, DEFAULT_DEBOUNCE_MS)
            .expect("Failed to create persistence actor");

        // Spawn state actor with persistence channel
        let state_actor = StateActorHandle::spawn(persistence_actor.cmd_tx());

        // Open sled database for camera target state (separate db to avoid lock conflicts)
        let camera_db_path = format!("{}_camera", db_path);
        let camera_db =
            sled::open(&camera_db_path).expect("Failed to open sled database for camera targets");
        let camera_targets = Arc::new(CameraTargetState::new(camera_db));

        Self {
            config: Arc::new(RwLock::new(config)),
            drivers: Arc::new(RwLock::new(HashMap::new())),
            active_page_index: Arc::new(RwLock::new(0)),
            state_actor,
            persistence_actor,
            display_needs_update: Arc::new(tokio::sync::Mutex::new(false)),
            fader_setpoint: Arc::new(fader_setpoint),
            setpoint_rx: Arc::new(tokio::sync::Mutex::new(Some(setpoint_rx))),
            pending_midi_messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            activity_tracker: None,
            page_epoch: Arc::new(AtomicU64::new(0)),
            camera_targets,
        }
    }

    /// Set the activity tracker for LED visualization
    pub fn set_activity_tracker(&mut self, tracker: Arc<crate::tray::ActivityTracker>) {
        self.activity_tracker = Some(tracker);
    }

    /// Check if display needs update after page change and reset flag
    pub async fn check_and_clear_display_update(&self) -> bool {
        let mut flag = self.display_needs_update.lock().await;
        let needs_update = *flag;
        *flag = false;
        needs_update
    }

    /// Take pending MIDI messages (consumes them, leaving empty Vec)
    pub async fn take_pending_midi(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.pending_midi_messages.lock().await)
    }

    /// Get current timestamp in milliseconds
    pub(crate) fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Get the fader setpoint scheduler (for applying setpoints)
    pub fn get_fader_setpoint(&self) -> Arc<FaderSetpoint> {
        self.fader_setpoint.clone()
    }

    /// Get the current page epoch (BUG-006 FIX)
    ///
    /// The epoch is incremented on each page change. Callers should capture this
    /// value before processing feedback and verify it's still current before
    /// applying state updates. This prevents stale feedback from contaminating
    /// the new page's state during page transitions.
    pub fn get_page_epoch(&self) -> u64 {
        self.page_epoch.load(Ordering::Acquire)
    }

    /// Check if a captured epoch is still current (BUG-006 FIX)
    ///
    /// Returns true if the epoch matches the current page epoch.
    /// Use this to verify feedback is still valid before applying state updates.
    pub fn is_epoch_current(&self, captured_epoch: u64) -> bool {
        self.page_epoch.load(Ordering::Acquire) == captured_epoch
    }

    /// Increment page epoch (called during page change)
    ///
    /// This invalidates all in-flight feedback processing.
    fn increment_page_epoch(&self) -> u64 {
        self.page_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Take the setpoint apply receiver (should only be called once by main loop)
    pub async fn take_setpoint_receiver(
        &self,
    ) -> Option<mpsc::UnboundedReceiver<ApplySetpointCmd>> {
        let mut rx_guard = self.setpoint_rx.lock().await;
        rx_guard.take()
    }

    /// Get reference to StateActorHandle (for state operations)
    pub fn get_state_actor(&self) -> &StateActorHandle {
        &self.state_actor
    }

    /// Get reference to PersistenceActorHandle (for loading/saving snapshots)
    pub fn get_persistence_actor(&self) -> &PersistenceActorHandle {
        &self.persistence_actor
    }

    /// Get reference to CameraTargetState (for API)
    pub fn get_camera_targets(&self) -> Arc<CameraTargetState> {
        self.camera_targets.clone()
    }

    /// Save current state to the persistence actor (debounced)
    ///
    /// This collects state from all apps and sends it to the persistence actor
    /// for debounced saving to sled.
    pub async fn save_state_snapshot(&self) -> Result<()> {
        use crate::state::{AppKey, StateSnapshot};

        // Collect states from all apps
        let all_apps: Vec<AppKey> = AppKey::all().to_vec();
        let states = self.state_actor.list_states_for_apps(all_apps).await;

        // Build snapshot
        let snapshot = StateSnapshot {
            timestamp: Self::now_ms(),
            version: StateSnapshot::VERSION.to_string(),
            states,
        };

        // Send to persistence actor (debounced)
        self.persistence_actor.save_snapshot(snapshot).await
    }

    /// Flush pending state snapshot to disk immediately
    ///
    /// Use this before shutdown to ensure state is persisted.
    pub async fn flush_state_snapshot(&self) -> Result<()> {
        self.persistence_actor.flush().await
    }

    /// Update configuration and notify drivers (hot-reload support)
    pub async fn update_config(&self, new_config: AppConfig) -> Result<()> {
        use tracing::{debug, info, warn};

        info!("🔄 Updating configuration (hot-reload)...");

        // Update config
        *self.config.write().await = new_config;

        // Ensure active page index is still valid
        let config = self.config.read().await;
        let mut index = self.active_page_index.write().await;
        let old_index = *index;
        if *index >= config.pages.len() {
            *index = 0;
            warn!(
                "Active page index {} out of range (config has {} pages), reset to 0",
                old_index,
                config.pages.len()
            );
        }
        drop(index);
        drop(config);

        // Notify all drivers to sync with new config
        let drivers = self.drivers.read().await;
        let driver_list: Vec<_> = drivers
            .iter()
            .map(|(name, driver)| (name.clone(), driver.clone()))
            .collect();
        drop(drivers);

        let mut sync_errors = Vec::new();
        for (name, driver) in driver_list {
            debug!("Syncing driver '{}' with new config...", name);
            if let Err(e) = driver.sync().await {
                warn!("Driver '{}' sync failed after config update: {}", name, e);
                sync_errors.push((name, e));
            } else {
                debug!("✅ Driver '{}' synced", name);
            }
        }

        if !sync_errors.is_empty() {
            warn!(
                "⚠️  {} driver(s) failed to sync after config update",
                sync_errors.len()
            );
        }

        // Refresh the active page to apply new mappings
        self.refresh_page().await;

        info!("✅ Configuration updated successfully");
        Ok(())
    }
}
