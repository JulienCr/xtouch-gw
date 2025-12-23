//! Router module - Core orchestration of MIDI events and page management
//!
//! The Router is the central orchestrator that manages:
//! - Page navigation and control mapping resolution
//! - MIDI state tracking per application
//! - Driver registration and execution
//! - Anti-echo windows and Last-Write-Wins logic
//! - Page refresh with state replay

mod anti_echo;
mod driver;
mod feedback;
mod indicators;
mod page;
mod refresh;
mod xtouch_input;

#[cfg(test)]
mod tests;

use crate::config::AppConfig;
use crate::drivers::Driver;
use crate::state::StateStore;
use crate::xtouch::fader_setpoint::{ApplySetpointCmd, FaderSetpoint};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};

// Re-export anti-echo types for internal use
pub(crate) use anti_echo::ShadowEntry;

/// Main router orchestrating page navigation, state management, and driver execution
pub struct Router {
    /// Application configuration
    pub(crate) config: Arc<RwLock<AppConfig>>,
    /// Registered drivers by name
    pub(crate) drivers: Arc<RwLock<HashMap<String, Arc<dyn Driver>>>>,
    /// Active page index
    pub(crate) active_page_index: Arc<RwLock<usize>>,
    /// MIDI state store (per application)
    pub(crate) state: StateStore,
    /// Shadow states per app (for anti-echo)
    pub(crate) app_shadows: Arc<StdRwLock<HashMap<String, HashMap<String, ShadowEntry>>>>,
    /// Last user action timestamps per X-Touch control (for Last-Write-Wins)
    pub(crate) last_user_action_ts: Arc<StdRwLock<HashMap<String, u64>>>,
    /// Flag indicating display needs update after page change
    pub(crate) display_needs_update: Arc<tokio::sync::Mutex<bool>>,
    /// Fader setpoint scheduler (motor position tracking)
    pub(crate) fader_setpoint: Arc<FaderSetpoint>,
    /// Receiver for setpoint apply commands (stored for retrieval)
    pub(crate) setpoint_rx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<ApplySetpointCmd>>>>,
    /// Pending MIDI messages to send to X-Touch (e.g., from page refresh)
    pub(crate) pending_midi_messages: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    /// Activity tracker for tray UI LED visualization
    pub(crate) activity_tracker: Option<Arc<crate::tray::ActivityTracker>>,
}

impl Router {
    /// Create a new Router with initial configuration
    pub fn new(config: AppConfig) -> Self {
        let (fader_setpoint, setpoint_rx) = FaderSetpoint::new();

        Self {
            config: Arc::new(RwLock::new(config)),
            drivers: Arc::new(RwLock::new(HashMap::new())),
            active_page_index: Arc::new(RwLock::new(0)),
            state: StateStore::new(),
            app_shadows: Arc::new(StdRwLock::new(HashMap::new())),
            last_user_action_ts: Arc::new(StdRwLock::new(HashMap::new())),
            display_needs_update: Arc::new(tokio::sync::Mutex::new(false)),
            fader_setpoint: Arc::new(fader_setpoint),
            setpoint_rx: Arc::new(tokio::sync::Mutex::new(Some(setpoint_rx))),
            pending_midi_messages: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            activity_tracker: None,
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

    /// Take the setpoint apply receiver (should only be called once by main loop)
    pub async fn take_setpoint_receiver(&self) -> Option<mpsc::UnboundedReceiver<ApplySetpointCmd>> {
        let mut rx_guard = self.setpoint_rx.lock().await;
        rx_guard.take()
    }

    /// Get reference to StateStore (for loading snapshots)
    pub fn get_state_store(&self) -> &StateStore {
        &self.state
    }

    /// Save state snapshot to disk
    pub async fn save_state_snapshot(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        self.state.save_snapshot(path).await
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

