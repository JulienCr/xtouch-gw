//! Router module - Core orchestration of MIDI events and page management
//!
//! The Router is the central orchestrator that manages:
//! - Page navigation and control mapping resolution
//! - MIDI state tracking per application
//! - Driver registration and execution
//! - Anti-echo windows and Last-Write-Wins logic
//! - Page refresh with state replay

use crate::config::{AppConfig, PageConfig};
use crate::drivers::{Driver, ExecutionContext};
use crate::state::{build_entry_from_raw, AppKey, MidiStateEntry, MidiStatus, StateStore};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};

/// Anti-echo time windows (in milliseconds) per MIDI status type
const ANTI_ECHO_WINDOWS: &[(MidiStatus, u64)] = &[
    (MidiStatus::PB, 250),    // Pitch Bend: motors need time to settle
    (MidiStatus::CC, 100),     // Control Change: encoders can generate rapid changes
    (MidiStatus::Note, 10),    // Note On/Off: buttons are discrete events
    (MidiStatus::SysEx, 60),   // SysEx: fallback for other messages
];

/// Last-Write-Wins grace periods (in milliseconds)
const LWW_GRACE_PERIOD_PB: u64 = 300;
const LWW_GRACE_PERIOD_CC: u64 = 50;

/// Shadow state entry (value + timestamp)
#[derive(Debug, Clone)]
struct ShadowEntry {
    value: u16,
    ts: u64,
}

impl ShadowEntry {
    fn new(value: u16) -> Self {
        Self {
            value,
            ts: Router::now_ms(),
        }
    }
}

/// Main router orchestrating page navigation, state management, and driver execution
pub struct Router {
    /// Application configuration
    config: Arc<RwLock<AppConfig>>,
    /// Registered drivers by name
    drivers: Arc<RwLock<HashMap<String, Arc<dyn Driver>>>>,
    /// Active page index
    active_page_index: Arc<RwLock<usize>>,
    /// MIDI state store (per application)
    state: StateStore,
    /// Shadow states per app (for anti-echo)
    app_shadows: Arc<StdRwLock<HashMap<String, HashMap<String, ShadowEntry>>>>,
    /// Last user action timestamps per X-Touch control (for Last-Write-Wins)
    last_user_action_ts: Arc<StdRwLock<HashMap<String, u64>>>,
}

impl Router {
    /// Create a new Router with initial configuration
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            drivers: Arc::new(RwLock::new(HashMap::new())),
            active_page_index: Arc::new(RwLock::new(0)),
            state: StateStore::new(),
            app_shadows: Arc::new(StdRwLock::new(HashMap::new())),
            last_user_action_ts: Arc::new(StdRwLock::new(HashMap::new())),
        }
    }

    /// Create an execution context for driver calls
    async fn create_execution_context(&self) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value: None,
        }
    }

    /// Register a driver by name (e.g., "voicemeeter", "qlc", "obs")
    /// 
    /// The driver will be initialized immediately upon registration
    pub async fn register_driver(&self, name: String, driver: Arc<dyn Driver>) -> Result<()> {
        info!("Registering driver '{}'...", name);

        // Create execution context
        let ctx = self.create_execution_context().await;

        // Initialize the driver
        if let Err(e) = driver.init(ctx).await {
            warn!("Failed to initialize driver '{}': {}", name, e);
            return Err(e);
        }

        // Store the driver
        let mut drivers = self.drivers.write().await;
        drivers.insert(name.clone(), driver);
        
        info!("✅ Driver '{}' registered and initialized", name);
        Ok(())
    }

    /// Get a driver by name
    pub async fn get_driver(&self, name: &str) -> Option<Arc<dyn Driver>> {
        let drivers = self.drivers.read().await;
        drivers.get(name).cloned()
    }

    /// List all registered driver names
    pub async fn list_drivers(&self) -> Vec<String> {
        let drivers = self.drivers.read().await;
        drivers.keys().cloned().collect()
    }

    /// Shutdown all registered drivers
    pub async fn shutdown_all_drivers(&self) -> Result<()> {
        info!("Shutting down all drivers...");
        
        let drivers = self.drivers.read().await;
        let driver_list: Vec<_> = drivers.iter().map(|(name, driver)| (name.clone(), driver.clone())).collect();
        drop(drivers);

        let mut errors = Vec::new();
        for (name, driver) in driver_list {
            info!("Shutting down driver '{}'...", name);
            if let Err(e) = driver.shutdown().await {
                warn!("Failed to shutdown driver '{}': {}", name, e);
                errors.push((name, e));
            } else {
                info!("✅ Driver '{}' shut down", name);
            }
        }

        if !errors.is_empty() {
            let error_msg = errors
                .iter()
                .map(|(n, e)| format!("{}: {}", n, e))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(anyhow!("Failed to shutdown {} driver(s): {}", errors.len(), error_msg));
        }

        // Clear the driver registry
        self.drivers.write().await.clear();
        info!("All drivers shut down successfully");
        Ok(())
    }

    /// Get the active page configuration
    pub async fn get_active_page(&self) -> Option<PageConfig> {
        let config = self.config.read().await;
        let index = *self.active_page_index.read().await;
        config.pages.get(index).cloned()
    }

    /// Get the active page name
    pub async fn get_active_page_name(&self) -> String {
        self.get_active_page()
            .await
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "(none)".to_string())
    }

    /// List all page names
    pub async fn list_pages(&self) -> Vec<String> {
        let config = self.config.read().await;
        config.pages.iter().map(|p| p.name.clone()).collect()
    }

    /// Set active page by index or name
    pub async fn set_active_page(&self, name_or_index: &str) -> Result<()> {
        let config = self.config.read().await;

        // Try parsing as index first
        if let Ok(index) = name_or_index.parse::<usize>() {
            if index < config.pages.len() {
                *self.active_page_index.write().await = index;
                let page_name = self.get_active_page_name().await;
                info!("Active page: {}", page_name);
                drop(config); // Release lock before refresh
                self.refresh_page().await;
                return Ok(());
            }
            return Err(anyhow!("Page index {} out of range", index));
        }

        // Try finding by name
        if let Some(index) = config
            .pages
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name_or_index))
        {
            *self.active_page_index.write().await = index;
            let page_name = self.get_active_page_name().await;
            info!("Active page: {}", page_name);
            drop(config); // Release lock before refresh
            self.refresh_page().await;
            return Ok(());
        }

        Err(anyhow!("Page '{}' not found", name_or_index))
    }

    /// Navigate to the next page (circular)
    pub async fn next_page(&self) {
        let config = self.config.read().await;
        if config.pages.is_empty() {
            return;
        }

        let mut index = self.active_page_index.write().await;
        *index = (*index + 1) % config.pages.len();
        let page_name = config.pages[*index].name.clone();
        info!("Next page → {}", page_name);
        drop(index);
        drop(config);

        self.refresh_page().await;
    }

    /// Navigate to the previous page (circular)
    pub async fn prev_page(&self) {
        let config = self.config.read().await;
        if config.pages.is_empty() {
            return;
        }

        let mut index = self.active_page_index.write().await;
        *index = if *index == 0 {
            config.pages.len() - 1
        } else {
            *index - 1
        };
        let page_name = config.pages[*index].name.clone();
        info!("Previous page → {}", page_name);
        drop(index);
        drop(config);

        self.refresh_page().await;
    }

    /// Handle a control event (resolve mapping and execute driver action)
    pub async fn handle_control(&self, control_id: &str, value: Option<Value>) -> Result<()> {
        let page = self
            .get_active_page()
            .await
            .ok_or_else(|| anyhow!("No active page"))?;

        // Look up the control mapping
        let mapping = page
            .controls
            .as_ref()
            .and_then(|controls| controls.get(control_id))
            .ok_or_else(|| anyhow!("No mapping for control '{}'", control_id))?;

        let app_name = &mapping.app;

        let action = mapping
            .action
            .as_ref()
            .ok_or_else(|| anyhow!("Control '{}' has no action defined", control_id))?;

        // Get the driver
        let driver = self
            .get_driver(app_name)
            .await
            .ok_or_else(|| anyhow!("Driver '{}' not registered", app_name))?;

        // Extract params
        let params = mapping.params.clone().unwrap_or_default();

        debug!(
            "Executing {}.{} for control '{}' (value: {:?})",
            app_name, action, control_id, value
        );

        // Create execution context
        let ctx = self.create_execution_context().await;

        // Execute the driver action
        driver.execute(action, params, ctx).await?;

        Ok(())
    }

    /// Process MIDI input from X-Touch (for page navigation)
    ///
    /// Handles navigation notes:
    /// - Note 46 (default): Previous page
    /// - Note 47 (default): Next page
    /// - Notes 54-61: Direct page access (F1-F8)
    pub async fn on_midi_from_xtouch(&self, raw: &[u8]) {
        if raw.len() < 3 {
            return;
        }

        let status = raw[0];
        let type_nibble = (status & 0xF0) >> 4;
        let channel = (status & 0x0F) + 1;

        // Only process Note On messages (0x9x)
        if type_nibble != 0x9 {
            return;
        }

        let note = raw[1];
        let velocity = raw[2];

        // Ignore Note Off (velocity 0)
        if velocity == 0 {
            return;
        }

        // Get paging configuration
        let config = self.config.read().await;
        let paging_channel = config
            .paging
            .as_ref()
            .map(|p| p.channel)
            .unwrap_or(1);
        let prev_note = config
            .paging
            .as_ref()
            .map(|p| p.prev_note)
            .unwrap_or(46);
        let next_note = config
            .paging
            .as_ref()
            .map(|p| p.next_note)
            .unwrap_or(47);
        drop(config);

        // Only process notes on the paging channel
        if channel != paging_channel {
            return;
        }

        // Check for prev/next navigation
        if note == prev_note {
            debug!("X-Touch: Previous page (note {})", note);
            self.prev_page().await;
            return;
        }

        if note == next_note {
            debug!("X-Touch: Next page (note {})", note);
            self.next_page().await;
            return;
        }

        // Check for F-key direct page access (F1-F8 = notes 54-61)
        if (54..=61).contains(&note) {
            let page_index = (note - 54) as usize;
            debug!("X-Touch: Direct page access F{} (note {})", page_index + 1, note);
            
            let config = self.config.read().await;
            if page_index < config.pages.len() {
                drop(config);
                let _ = self.set_active_page(&page_index.to_string()).await;
            }
        }
    }

    /// Refresh the active page (replay all known states to X-Touch)
    async fn refresh_page(&self) {
        let page = match self.get_active_page().await {
            Some(p) => p,
            None => return,
        };

        debug!("Refreshing page '{}'", page.name);

        // Clear X-Touch shadow state to allow re-emission
        self.clear_xtouch_shadow();

        // Build and execute refresh plan
        let entries = self.plan_page_refresh(&page);
        
        debug!(
            "Page refresh plan: {} Notes, {} CCs, {} PBs",
            entries.iter().filter(|e| e.addr.status == MidiStatus::Note).count(),
            entries.iter().filter(|e| e.addr.status == MidiStatus::CC).count(),
            entries.iter().filter(|e| e.addr.status == MidiStatus::PB).count()
        );

        // TODO: Send entries to X-Touch via MIDI output
        // This will be implemented in Phase 4 when we have XTouchDriver integration

        info!("Page refresh completed: {} (planned {} entries)", page.name, entries.len());
    }

    /// Clear X-Touch shadow state (allows re-emission during refresh)
    fn clear_xtouch_shadow(&self) {
        // X-Touch shadow is per-app, clear all
        if let Ok(mut shadows) = self.app_shadows.write() {
            shadows.clear();
        }
    }

    /// Plan page refresh: build ordered list of MIDI entries to send
    /// 
    /// Returns entries in order: Notes → CC → SysEx → PB
    /// Priority for each type:
    /// - PB: Known PB = 3 > Mapped CC = 2 > Zero = 1
    /// - Notes/CC: Known value = 2 > Reset (0/OFF) = 1
    fn plan_page_refresh(&self, _page: &PageConfig) -> Vec<MidiStateEntry> {
        use crate::state::{MidiAddr, MidiValue, Origin};

        #[derive(Clone)]
        struct PlanEntry {
            entry: MidiStateEntry,
            priority: u8,
        }

        let mut note_plan: HashMap<String, PlanEntry> = HashMap::new();
        let mut cc_plan: HashMap<String, PlanEntry> = HashMap::new();
        let mut pb_plan: HashMap<u8, PlanEntry> = HashMap::new();

        // Helper to push candidates with priority
        let push_note = |map: &mut HashMap<String, PlanEntry>, e: MidiStateEntry, prio: u8| {
            let key = format!("{}|{}", e.addr.channel.unwrap_or(0), e.addr.data1.unwrap_or(0));
            let should_insert = match map.get(&key) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(key, PlanEntry { entry: e, priority: prio });
            }
        };

        let push_cc = |map: &mut HashMap<String, PlanEntry>, e: MidiStateEntry, prio: u8| {
            let key = format!("{}|{}", e.addr.channel.unwrap_or(0), e.addr.data1.unwrap_or(0));
            let should_insert = match map.get(&key) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(key, PlanEntry { entry: e, priority: prio });
            }
        };

        let push_pb = |map: &mut HashMap<u8, PlanEntry>, ch: u8, e: MidiStateEntry, prio: u8| {
            let should_insert = match map.get(&ch) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(ch, PlanEntry { entry: e, priority: prio });
            }
        };

        // For simplicity, assume all apps use channels 1-9
        // In a full implementation, this would check page.passthroughs for actual channels
        let channels: Vec<u8> = (1..=9).collect();

        // Build plans for each app
        for app in AppKey::all() {
            // PB plan (priority: Known PB = 3 > Mapped CC = 2 > Zero = 1)
            for &ch in &channels {
                // Try to get known PB value
                if let Some(latest_pb) = self.state.get_known_latest_for_app(*app, MidiStatus::PB, Some(ch), Some(0)) {
                    push_pb(&mut pb_plan, ch, latest_pb, 3);
                    continue;
                }

                // Fallback: send zero
                let zero_pb = MidiStateEntry {
                    addr: MidiAddr {
                        port_id: app.as_str().to_string(),
                        status: MidiStatus::PB,
                        channel: Some(ch),
                        data1: Some(0),
                    },
                    value: MidiValue::Number(0),
                    ts: Self::now_ms(),
                    origin: Origin::XTouch,
                    known: false,
                    stale: false,
                    hash: None,
                };
                push_pb(&mut pb_plan, ch, zero_pb, 1);
            }

            // Notes: 0-31 (priority: Known = 2 > Reset OFF = 1)
            for &ch in &channels {
                for note in 0..=31 {
                    if let Some(latest_note) = self.state.get_known_latest_for_app(*app, MidiStatus::Note, Some(ch), Some(note)) {
                        push_note(&mut note_plan, latest_note, 2);
                    } else {
                        // Reset to OFF
                        let off = MidiStateEntry {
                            addr: MidiAddr {
                                port_id: app.as_str().to_string(),
                                status: MidiStatus::Note,
                                channel: Some(ch),
                                data1: Some(note),
                            },
                            value: MidiValue::Number(0),
                            ts: Self::now_ms(),
                            origin: Origin::XTouch,
                            known: false,
                            stale: false,
                            hash: None,
                        };
                        push_note(&mut note_plan, off, 1);
                    }
                }
            }

            // CC (rings): 0-31 (priority: Known = 2 > Reset 0 = 1)
            for &ch in &channels {
                for cc in 0..=31 {
                    if let Some(latest_cc) = self.state.get_known_latest_for_app(*app, MidiStatus::CC, Some(ch), Some(cc)) {
                        push_cc(&mut cc_plan, latest_cc, 2);
                    } else {
                        // Reset to 0
                        let zero = MidiStateEntry {
                            addr: MidiAddr {
                                port_id: app.as_str().to_string(),
                                status: MidiStatus::CC,
                                channel: Some(ch),
                                data1: Some(cc),
                            },
                            value: MidiValue::Number(0),
                            ts: Self::now_ms(),
                            origin: Origin::XTouch,
                            known: false,
                            stale: false,
                            hash: None,
                        };
                        push_cc(&mut cc_plan, zero, 1);
                    }
                }
            }
        }

        // Materialize plans into ordered list: Notes → CC → PB
        let mut entries = Vec::new();
        
        // Notes first
        for plan_entry in note_plan.values() {
            entries.push(plan_entry.entry.clone());
        }
        
        // Then CC
        for plan_entry in cc_plan.values() {
            entries.push(plan_entry.entry.clone());
        }
        
        // Finally PB
        for plan_entry in pb_plan.values() {
            entries.push(plan_entry.entry.clone());
        }

        entries
    }

    /// Update configuration and notify drivers (hot-reload support)
    pub async fn update_config(&self, new_config: AppConfig) -> Result<()> {
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
        let driver_list: Vec<_> = drivers.iter().map(|(name, driver)| (name.clone(), driver.clone())).collect();
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

    /// Get current timestamp in milliseconds
    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Get anti-echo window for a MIDI status type
    fn get_anti_echo_window(status: MidiStatus) -> u64 {
        ANTI_ECHO_WINDOWS
            .iter()
            .find(|(s, _)| *s == status)
            .map(|(_, ms)| *ms)
            .unwrap_or(60)
    }

    /// Check if a value should be suppressed due to anti-echo
    fn should_suppress_anti_echo(&self, app_key: &str, entry: &MidiStateEntry) -> bool {
        let app_shadows = match self.app_shadows.try_read() {
            Ok(shadows) => shadows,
            Err(_) => return false,
        };

        let app_shadow = match app_shadows.get(app_key) {
            Some(shadow) => shadow,
            None => return false,
        };

        let key = format!(
            "{}|{}|{}",
            entry.addr.status,
            entry.addr.channel.unwrap_or(0),
            entry.addr.data1.unwrap_or(0)
        );

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
    fn update_app_shadow(&self, app_key: &str, entry: &MidiStateEntry) {
        let key = format!(
            "{}|{}|{}",
            entry.addr.status,
            entry.addr.channel.unwrap_or(0),
            entry.addr.data1.unwrap_or(0)
        );

        let value = entry.value.as_number().unwrap_or(0);
        let shadow_entry = ShadowEntry::new(value);

        let mut app_shadows = self.app_shadows.write().unwrap();
        let app_shadow = app_shadows.entry(app_key.to_string()).or_insert_with(HashMap::new);
        app_shadow.insert(key, shadow_entry);
    }

    /// Check Last-Write-Wins: should suppress feedback if user action was recent
    fn should_suppress_lww(&self, entry: &MidiStateEntry) -> bool {
        let key = format!(
            "{}|{}|{}",
            entry.addr.status,
            entry.addr.channel.unwrap_or(0),
            entry.addr.data1.unwrap_or(0)
        );

        let last_user_ts = self.last_user_action_ts.try_read()
            .ok()
            .and_then(|ts_map| ts_map.get(&key).copied())
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
                format!("note|{}|{}", channel, note)
            }
            0xB => {
                // Control Change
                let cc = raw.get(1).copied().unwrap_or(0);
                format!("cc|{}|{}", channel, cc)
            }
            0xE => {
                // Pitch Bend
                format!("pb|{}|0", channel)
            }
            _ => return,
        };

        let mut ts_map = self.last_user_action_ts.write().unwrap();
        ts_map.insert(key, Self::now_ms());
    }

    /// Ingest MIDI feedback from an application
    ///
    /// This is the entry point for feedback from applications (OBS, Voicemeeter, etc.).
    /// It updates the state store and forwards to X-Touch if relevant for the active page.
    ///
    /// # Arguments
    ///
    /// * `app_key` - Application identifier (e.g., "obs", "voicemeeter")
    /// * `raw` - Raw MIDI bytes from the application
    /// * `port_id` - MIDI port identifier
    pub fn on_midi_from_app(&self, app_key: &str, raw: &[u8], port_id: &str) {
        // Parse the MIDI message
        let entry = match build_entry_from_raw(raw, port_id) {
            Some(e) => e,
            None => {
                debug!("Failed to parse MIDI from app '{}': {:02X?}", app_key, raw);
                return;
            }
        };

        // Update state store (this also notifies subscribers)
        let app = match AppKey::from_str(app_key) {
            Some(a) => a,
            None => {
                warn!("Unknown application key: {}", app_key);
                return;
            }
        };

        // Update state store
        self.state.update_from_feedback(app, entry.clone());

        // Log for debugging
        trace!(
            "State <- {}: status={:?} ch={:?} d1={:?} val={:?}",
            app_key,
            entry.addr.status,
            entry.addr.channel,
            entry.addr.data1,
            entry.value
        );

        // Mark this as sent to app (for anti-echo)
        self.update_app_shadow(app_key, &entry);

        // TODO: Forward to X-Touch if relevant for active page
        // This will be implemented in the forward module (Phase 6.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MidiConfig;

    fn make_test_config(pages: Vec<PageConfig>) -> AppConfig {
        AppConfig {
            midi: MidiConfig {
                input_port: "test_in".to_string(),
                output_port: "test_out".to_string(),
                apps: None,
            },
            obs: None,
            xtouch: None,
            paging: None,
            gamepad: None,
            pages_global: None,
            pages,
        }
    }

    fn make_test_page(name: &str) -> PageConfig {
        PageConfig {
            name: name.to_string(),
            controls: None,
            lcd: None,
            passthrough: None,
            passthroughs: None,
        }
    }

    #[tokio::test]
    async fn test_page_navigation() {
        let config = make_test_config(vec![
            make_test_page("Page 1"),
            make_test_page("Page 2"),
            make_test_page("Page 3"),
        ]);

        let router = Router::new(config);

        assert_eq!(router.get_active_page_name().await, "Page 1");

        router.next_page().await;
        assert_eq!(router.get_active_page_name().await, "Page 2");

        router.next_page().await;
        assert_eq!(router.get_active_page_name().await, "Page 3");

        router.next_page().await; // Wrap around
        assert_eq!(router.get_active_page_name().await, "Page 1");

        router.prev_page().await; // Wrap around backwards
        assert_eq!(router.get_active_page_name().await, "Page 3");
    }

    #[tokio::test]
    async fn test_set_page_by_name() {
        let config = make_test_config(vec![
            make_test_page("Voicemeeter"),
            make_test_page("OBS"),
        ]);

        let router = Router::new(config);

        router.set_active_page("OBS").await.unwrap();
        assert_eq!(router.get_active_page_name().await, "OBS");

        router.set_active_page("voicemeeter").await.unwrap(); // Case insensitive
        assert_eq!(router.get_active_page_name().await, "Voicemeeter");
    }

    #[tokio::test]
    async fn test_set_page_by_index() {
        let config = make_test_config(vec![
            make_test_page("Page 0"),
            make_test_page("Page 1"),
        ]);

        let router = Router::new(config);

        router.set_active_page("1").await.unwrap();
        assert_eq!(router.get_active_page_name().await, "Page 1");

        router.set_active_page("0").await.unwrap();
        assert_eq!(router.get_active_page_name().await, "Page 0");
    }

    #[tokio::test]
    async fn test_midi_note_navigation() {
        let config = make_test_config(vec![
            make_test_page("Page 1"),
            make_test_page("Page 2"),
            make_test_page("Page 3"),
        ]);

        let router = Router::new(config);

        // Test next page (note 47 on channel 1)
        let note_on_next = [0x90, 47, 127]; // Note On, Ch1, note 47, velocity 127
        router.on_midi_from_xtouch(&note_on_next).await;
        assert_eq!(router.get_active_page_name().await, "Page 2");

        // Test prev page (note 46 on channel 1)
        let note_on_prev = [0x90, 46, 127]; // Note On, Ch1, note 46, velocity 127
        router.on_midi_from_xtouch(&note_on_prev).await;
        assert_eq!(router.get_active_page_name().await, "Page 1");

        // Test F-key direct access (F3 = note 56 = page index 2)
        let note_on_f3 = [0x90, 56, 127]; // Note On, Ch1, note 56 (F3)
        router.on_midi_from_xtouch(&note_on_f3).await;
        assert_eq!(router.get_active_page_name().await, "Page 3");
    }

    #[tokio::test]
    async fn test_midi_note_navigation_ignores_velocity_zero() {
        let config = make_test_config(vec![
            make_test_page("Page 1"),
            make_test_page("Page 2"),
        ]);

        let router = Router::new(config);

        // Note Off (velocity 0) should be ignored
        let note_off = [0x90, 47, 0]; // Note On with velocity 0 = Note Off
        router.on_midi_from_xtouch(&note_off).await;
        assert_eq!(router.get_active_page_name().await, "Page 1"); // Should stay on Page 1
    }

    // ===== PHASE 4: Driver Framework Integration Tests =====

    #[tokio::test]
    async fn test_driver_registration_and_initialization() {
        use crate::drivers::ConsoleDriver;
        use std::sync::Arc;

        let config = make_test_config(vec![make_test_page("Test Page")]);
        let router = Router::new(config);

        // Create and register a console driver
        let driver = Arc::new(ConsoleDriver::new("test_console"));
        let result = router.register_driver("test_console".to_string(), driver).await;

        assert!(result.is_ok());

        // Verify driver is registered
        let driver_names = router.list_drivers().await;
        assert!(driver_names.contains(&"test_console".to_string()));

        // Verify we can retrieve the driver
        let retrieved = router.get_driver("test_console").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name(), "test_console");
    }

    #[tokio::test]
    async fn test_driver_shutdown_all() {
        use crate::drivers::ConsoleDriver;
        use std::sync::Arc;

        let config = make_test_config(vec![make_test_page("Test Page")]);
        let router = Router::new(config);

        // Register multiple drivers
        router
            .register_driver("driver1".to_string(), Arc::new(ConsoleDriver::new("driver1")))
            .await
            .unwrap();
        
        router
            .register_driver("driver2".to_string(), Arc::new(ConsoleDriver::new("driver2")))
            .await
            .unwrap();

        // Verify they're registered
        assert_eq!(router.list_drivers().await.len(), 2);

        // Shutdown all
        let result = router.shutdown_all_drivers().await;
        assert!(result.is_ok());

        // Verify all drivers are removed
        assert_eq!(router.list_drivers().await.len(), 0);
    }

    #[tokio::test]
    async fn test_driver_hot_reload_config() {
        use crate::drivers::ConsoleDriver;
        use std::sync::Arc;

        let initial_config = make_test_config(vec![
            make_test_page("Page 1"),
            make_test_page("Page 2"),
        ]);
        
        let router = Router::new(initial_config);

        // Register a driver
        router
            .register_driver("test_driver".to_string(), Arc::new(ConsoleDriver::new("test_driver")))
            .await
            .unwrap();

        // Update config with different pages
        let new_config = make_test_config(vec![
            make_test_page("New Page 1"),
            make_test_page("New Page 2"),
            make_test_page("New Page 3"),
        ]);

        let result = router.update_config(new_config).await;
        assert!(result.is_ok());

        // Verify new config is active
        let pages = router.list_pages().await;
        assert_eq!(pages.len(), 3);
        assert!(pages.contains(&"New Page 1".to_string()));
        assert!(pages.contains(&"New Page 3".to_string()));

        // Driver should still be registered
        assert!(router.get_driver("test_driver").await.is_some());
    }

    #[tokio::test]
    async fn test_driver_execution_with_context() {
        use crate::config::ControlMapping;
        use crate::drivers::ConsoleDriver;
        use serde_json::json;
        use std::collections::HashMap;
        use std::sync::Arc;

        // Create a page with control mappings
        let mut page = make_test_page("Test Page");
        let mut controls = HashMap::new();
        controls.insert(
            "fader1".to_string(),
            ControlMapping {
                app: "test_console".to_string(),
                action: Some("set_volume".to_string()),
                params: Some(vec![json!(100)]),
                midi: None,
                overlay: None,
                indicator: None,
            },
        );
        page.controls = Some(controls);

        let config = make_test_config(vec![page]);
        let router = Router::new(config);

        // Register driver
        router
            .register_driver("test_console".to_string(), Arc::new(ConsoleDriver::new("test_console")))
            .await
            .unwrap();

        // Execute control action
        let result = router.handle_control("fader1", Some(json!(127))).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_driver_execution_missing_driver() {
        use crate::config::ControlMapping;
        use serde_json::json;
        use std::collections::HashMap;

        // Create a page with control mapping pointing to non-existent driver
        let mut page = make_test_page("Test Page");
        let mut controls = HashMap::new();
        controls.insert(
            "fader1".to_string(),
            ControlMapping {
                app: "missing_driver".to_string(),
                action: Some("test_action".to_string()),
                params: None,
                midi: None,
                overlay: None,
                indicator: None,
            },
        );
        page.controls = Some(controls);

        let config = make_test_config(vec![page]);
        let router = Router::new(config);

        // Attempt to execute control action (should fail)
        let result = router.handle_control("fader1", Some(json!(127))).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Driver 'missing_driver' not registered"));
    }

    #[tokio::test]
    async fn test_driver_execution_missing_control() {
        use crate::drivers::ConsoleDriver;
        use serde_json::json;
        use std::sync::Arc;

        let config = make_test_config(vec![make_test_page("Test Page")]);
        let router = Router::new(config);

        // Register driver
        router
            .register_driver("test_console".to_string(), Arc::new(ConsoleDriver::new("test_console")))
            .await
            .unwrap();

        // Attempt to execute non-existent control
        let result = router.handle_control("non_existent_control", Some(json!(127))).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No mapping for control"));
    }

    #[tokio::test]
    async fn test_multiple_drivers_execution() {
        use crate::config::ControlMapping;
        use crate::drivers::ConsoleDriver;
        use serde_json::json;
        use std::collections::HashMap;
        use std::sync::Arc;

        // Create a page with multiple control mappings to different drivers
        let mut page = make_test_page("Multi Driver Page");
        let mut controls = HashMap::new();
        
        controls.insert(
            "obs_control".to_string(),
            ControlMapping {
                app: "obs_driver".to_string(),
                action: Some("switch_scene".to_string()),
                params: Some(vec![json!("Scene 1")]),
                midi: None,
                overlay: None,
                indicator: None,
            },
        );
        
        controls.insert(
            "vm_control".to_string(),
            ControlMapping {
                app: "vm_driver".to_string(),
                action: Some("set_fader".to_string()),
                params: Some(vec![json!(1), json!(0.5)]),
                midi: None,
                overlay: None,
                indicator: None,
            },
        );
        
        page.controls = Some(controls);

        let config = make_test_config(vec![page]);
        let router = Router::new(config);

        // Register multiple drivers
        router
            .register_driver("obs_driver".to_string(), Arc::new(ConsoleDriver::new("obs_driver")))
            .await
            .unwrap();
        
        router
            .register_driver("vm_driver".to_string(), Arc::new(ConsoleDriver::new("vm_driver")))
            .await
            .unwrap();

        // Execute controls for different drivers
        let result1 = router.handle_control("obs_control", None).await;
        assert!(result1.is_ok());

        let result2 = router.handle_control("vm_control", Some(json!(64))).await;
        assert!(result2.is_ok());

        // Verify both drivers are still registered
        assert_eq!(router.list_drivers().await.len(), 2);
    }
}


