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
use crate::xtouch::fader_setpoint::{ApplySetpointCmd, FaderSetpoint};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, trace, warn};

/// Anti-echo time windows (in milliseconds) per MIDI status type
const ANTI_ECHO_WINDOWS: &[(MidiStatus, u64)] = &[
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
    /// Flag indicating display needs update after page change
    display_needs_update: Arc<tokio::sync::Mutex<bool>>,
    /// Fader setpoint scheduler (motor position tracking)
    fader_setpoint: Arc<FaderSetpoint>,
    /// Receiver for setpoint apply commands (stored for retrieval)
    setpoint_rx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<ApplySetpointCmd>>>>,
    /// Pending MIDI messages to send to X-Touch (e.g., from page refresh)
    pending_midi_messages: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
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
        }
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

    /// Create an execution context for driver calls
    async fn create_execution_context(&self) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value: None,
            control_id: None,
        }
    }

    /// Create an execution context with control information
    async fn create_execution_context_with_control(&self, control_id: String, value: Option<serde_json::Value>) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value,
            control_id: Some(control_id),
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
        let driver_list: Vec<_> = drivers
            .iter()
            .map(|(name, driver)| (name.clone(), driver.clone()))
            .collect();
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
            return Err(anyhow!(
                "Failed to shutdown {} driver(s): {}",
                errors.len(),
                error_msg
            ));
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

        // Look up the control mapping - check page-specific controls first, then global controls
        let config = self.config.read().await;
        let mapping = page
            .controls
            .as_ref()
            .and_then(|controls| controls.get(control_id))
            .or_else(|| {
                config
                    .pages_global
                    .as_ref()
                    .and_then(|pg| pg.controls.as_ref())
                    .and_then(|controls| controls.get(control_id))
            })
            .ok_or_else(|| anyhow!("No mapping for control '{}'", control_id))?;

        // Clone data we need before dropping config lock
        let app_name = mapping.app.clone();
        let action = mapping
            .action
            .clone()
            .ok_or_else(|| anyhow!("Control '{}' has no action defined", control_id))?;
        let params = mapping.params.clone().unwrap_or_default();

        // Drop config lock before async operations
        drop(config);

        // Get the driver
        let driver = self
            .get_driver(&app_name)
            .await
            .ok_or_else(|| anyhow!("Driver '{}' not registered", app_name))?;

        debug!(
            "Executing {}.{} for control '{}' (value: {:?})",
            app_name, action, control_id, value
        );

        // Create execution context with control information
        let ctx = self.create_execution_context_with_control(control_id.to_string(), value).await;

        // Execute the driver action
        driver.execute(&action, params, ctx).await?;

        Ok(())
    }

    /// Process MIDI input from X-Touch hardware
    ///
    /// Handles:
    /// - Page navigation (F1-F8, prev/next buttons)
    /// - Control routing (faders, buttons, encoders → drivers)
    pub async fn on_midi_from_xtouch(&self, raw: &[u8]) {
        use crate::control_mapping::{load_default_mappings, MidiSpec};

        if raw.len() < 2 {
            return;
        }

        let status = raw[0];
        let type_nibble = (status & 0xF0) >> 4;
        let channel = (status & 0x0F) + 1;

        // First, check for page navigation (Note On messages only)
        if type_nibble == 0x9 && raw.len() >= 3 {
            let note = raw[1];
            let velocity = raw[2];

            // Ignore Note Off (velocity 0)
            if velocity != 0 {
                // Get paging configuration
                let config = self.config.read().await;
                let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1);
                let prev_note = config.paging.as_ref().map(|p| p.prev_note).unwrap_or(46);
                let next_note = config.paging.as_ref().map(|p| p.next_note).unwrap_or(47);
                drop(config);

                // Only process navigation on the paging channel
                if channel == paging_channel {
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
                        debug!(
                            "X-Touch: Direct page access F{} (note {})",
                            page_index + 1,
                            note
                        );

                        let config = self.config.read().await;
                        if page_index < config.pages.len() {
                            drop(config);
                            let _ = self.set_active_page(&page_index.to_string()).await;
                        }
                        return;
                    }
                }
            }
        }

        // Route control events to drivers
        let config = self.config.read().await;
        let is_mcu_mode = config
            .xtouch
            .as_ref()
            .map(|x| matches!(x.mode, crate::config::XTouchMode::Mcu))
            .unwrap_or(true); // Default to MCU mode
        drop(config);

        // Load control mappings
        let mapping_db = match load_default_mappings() {
            Ok(db) => db,
            Err(e) => {
                warn!("Failed to load control mappings: {}", e);
                return;
            },
        };

        // Parse incoming MIDI to determine which control was triggered
        let midi_spec = match MidiSpec::from_raw(raw) {
            Ok(spec) => spec,
            Err(_) => {
                trace!("Unsupported MIDI message: {:02X?}", raw);
                return;
            },
        };

        // Find the control ID from MIDI message
        let control_id = match mapping_db.find_control_by_midi(&midi_spec, is_mcu_mode) {
            Some(id) => id,
            None => {
                trace!("No control mapping found for MIDI: {:?}", midi_spec);
                return;
            },
        };

        debug!(
            "X-Touch control triggered: {} (MIDI: {:?})",
            control_id, midi_spec
        );

        // Mark user action for Last-Write-Wins
        self.mark_user_action(raw);

        // CRITICAL: Update fader setpoint for user actions (PitchBend from X-Touch)
        // This ensures the motor tracks the user's physical position
        if type_nibble == 0xE && raw.len() >= 3 {
            // PitchBend message
            let lsb = raw[1];
            let msb = raw[2];
            let value14 = (((msb as u16) << 7) | (lsb as u16)) as u16;
            debug!(
                "← User moved fader: ch={} value14={}",
                channel, value14
            );
            self.fader_setpoint.schedule(channel as u8, value14, None);
        }

        // Get active page and find control configuration
        let page = match self.get_active_page().await {
            Some(p) => p,
            None => {
                warn!("No active page");
                return;
            },
        };

        // Check page-specific controls first, then global controls as fallback
        let config = self.config.read().await;
        let control_config = page.controls
            .as_ref()
            .and_then(|controls| controls.get(control_id))
            .or_else(|| {
                config
                    .pages_global
                    .as_ref()
                    .and_then(|pg| pg.controls.as_ref())
                    .and_then(|controls| controls.get(control_id))
            });

        let control_config = match control_config {
            Some(cc) => cc.clone(),
            None => {
                trace!(
                    "Control '{}' not mapped on page '{}'",
                    control_id,
                    page.name
                );
                return;
            },
        };
        drop(config);

        // Check if this is MIDI direct mode (send raw MIDI to bridge)
        if let Some(target_spec) = &control_config.midi {
            // MIDI direct mode: transform and send to the app's bridge

            // 1. Parse input MIDI to get normalized value (0.0 - 1.0)
            let input_msg = crate::midi::MidiMessage::parse(raw);
            let normalized_value = match input_msg {
                Some(msg) => match msg {
                    crate::midi::MidiMessage::PitchBend { value, .. } => {
                        crate::midi::convert::to_percent_14bit(value) / 100.0
                    },
                    crate::midi::MidiMessage::ControlChange { value, .. } => {
                        crate::midi::convert::to_percent_7bit(value) / 100.0
                    },
                    crate::midi::MidiMessage::NoteOn { velocity, .. } => {
                        crate::midi::convert::to_percent_7bit(velocity) / 100.0
                    },
                    _ => 0.0,
                },
                None => 0.0,
            };

            // 2. Construct target message based on config
            let target_msg = match target_spec.midi_type {
                crate::config::MidiType::Cc => {
                    if let (Some(ch), Some(cc)) = (target_spec.channel, target_spec.cc) {
                        let value =
                            crate::midi::convert::from_percent_7bit(normalized_value * 100.0);
                        Some(crate::midi::MidiMessage::ControlChange {
                            channel: ch.saturating_sub(1), // Config is 1-based, internal is 0-based
                            cc,
                            value,
                        })
                    } else {
                        None
                    }
                },
                crate::config::MidiType::Note => {
                    if let (Some(ch), Some(note)) = (target_spec.channel, target_spec.note) {
                        let velocity =
                            crate::midi::convert::from_percent_7bit(normalized_value * 100.0);
                        // If velocity is 0, send NoteOff, otherwise NoteOn
                        if velocity == 0 {
                            Some(crate::midi::MidiMessage::NoteOff {
                                channel: ch.saturating_sub(1),
                                note,
                                velocity: 0,
                            })
                        } else {
                            Some(crate::midi::MidiMessage::NoteOn {
                                channel: ch.saturating_sub(1),
                                note,
                                velocity,
                            })
                        }
                    } else {
                        None
                    }
                },
                crate::config::MidiType::Pb => {
                    if let Some(ch) = target_spec.channel {
                        let value =
                            crate::midi::convert::from_percent_14bit(normalized_value * 100.0);
                        Some(crate::midi::MidiMessage::PitchBend {
                            channel: ch.saturating_sub(1),
                            value,
                        })
                    } else {
                        None
                    }
                },
                crate::config::MidiType::Passthrough => {
                    // Raw passthrough (no transformation)
                    crate::midi::MidiMessage::parse(raw)
                },
            };

            if let Some(msg) = target_msg {
                let bytes = msg.to_bytes();
                debug!(
                    "→ Transform: {} -> {} ({} bytes) to '{}'",
                    control_id,
                    msg,
                    bytes.len(),
                    control_config.app
                );

                // Get the MIDI bridge driver for this app
                let driver = {
                    let drivers = self.drivers.read().await;
                    drivers.get(&control_config.app).cloned()
                };

                if let Some(driver) = driver {
                    // Create execution context with transformed MIDI
                    let mut ctx = self.create_execution_context().await;
                    ctx.value = Some(match serde_json::to_value(&bytes) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("Failed to serialize MIDI data: {}", e);
                            return;
                        },
                    });

                    // Call passthrough action on the bridge
                    if let Err(e) = driver.execute("passthrough", vec![], ctx).await {
                        warn!("Failed to passthrough MIDI: {}", e);
                    }
                } else {
                    warn!(
                        "Bridge driver '{}' not found for MIDI passthrough",
                        control_config.app
                    );
                }
            }
            return;
        }

        // Driver action mode
        let driver = {
            let drivers = self.drivers.read().await;
            drivers.get(&control_config.app).cloned()
        };

        let driver = match driver {
            Some(d) => d,
            None => {
                warn!(
                    "Driver '{}' not found for control '{}'",
                    control_config.app, control_id
                );
                return;
            },
        };

        // Determine the action to execute
        let action = control_config.action.as_deref().unwrap_or("execute");

        // Build parameters
        let params = control_config.params.clone().unwrap_or_default();

        // Filter button releases (Note On with velocity 0)
        // This prevents toggle actions from firing twice (press + release)
        let status = raw[0];
        let type_nibble = (status & 0xF0) >> 4;
        if type_nibble == 0x9 && raw.len() >= 3 {
            let velocity = raw[2];
            if velocity == 0 {
                debug!("Ignoring Note Off (velocity 0) for control '{}'", control_id);
                return;
            }
        }

        // Create execution context with parsed MIDI value
        let mut ctx = self.create_execution_context().await;

        // Set control ID so drivers can detect input source (gamepad vs encoder)
        ctx.control_id = Some(control_id.to_string());

        // Parse MIDI message to extract the value/velocity byte
        // This allows drivers to receive a Number instead of raw bytes array
        if raw.len() >= 3 {
            let value = match type_nibble {
                0x9 => raw[2] as f64,        // Note On: velocity
                0xB => raw[2] as f64,        // Control Change: value
                0xE => {                      // Pitch Bend: 14-bit value (0-16383)
                    let lsb = raw[1] as u16;
                    let msb = raw[2] as u16;
                    ((msb << 7) | lsb) as f64
                },
                _ => {
                    // Unknown message type, pass raw bytes
                    match serde_json::to_value(raw) {
                        Ok(v) => {
                            ctx.value = Some(v);
                            0.0 // unused
                        },
                        Err(e) => {
                            warn!("Failed to serialize MIDI data: {}", e);
                            return;
                        },
                    }
                },
            };

            if type_nibble == 0x9 || type_nibble == 0xB || type_nibble == 0xE {
                ctx.value = Some(Value::Number(serde_json::Number::from_f64(value).unwrap()));
            }
        } else {
            // Short message, pass raw bytes
            ctx.value = Some(match serde_json::to_value(raw) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to serialize MIDI data: {}", e);
                    return;
                },
            });
        }

        // Execute driver action
        debug!(
            "→ Routing: {} → app={} action={} (value={:?})",
            control_id, control_config.app, action, ctx.value
        );
        if let Err(e) = driver.execute(action, params, ctx).await {
            warn!("Driver execution failed: {}", e);
        }
    }

    /// Get all apps that are active on a given page
    ///
    /// This includes apps referenced in:
    /// 1. Page-specific controls (`page.controls.*.app`)
    /// 2. Global controls (`pages_global.controls.*.app`)
    /// 3. Passthrough configurations (TODO)
    ///
    /// Used for page-aware feedback filtering (matches TypeScript getAppsForPage)
    fn get_apps_for_page(&self, page: &crate::config::PageConfig, config: &crate::config::AppConfig) -> std::collections::HashSet<String> {
        use std::collections::HashSet;

        let mut apps = HashSet::new();

        // 1. Extract apps from page-specific controls
        if let Some(controls) = &page.controls {
            for (_, mapping) in controls {
                apps.insert(mapping.app.clone());
            }
        }

        // 2. Extract apps from global controls (always available on all pages)
        if let Some(global) = &config.pages_global {
            if let Some(controls) = &global.controls {
                for (_, mapping) in controls {
                    apps.insert(mapping.app.clone());
                }
            }
        }

        // 3. TODO: Extract apps from passthrough configurations
        // if let Some(passthroughs) = &page.passthroughs {
        //     for pt in passthroughs {
        //         apps.insert(pt.app.clone());
        //     }
        // }

        apps
    }

    /// Process feedback from an application (reverse transformation)
    pub async fn process_feedback(&self, app_name: &str, raw_data: &[u8]) -> Option<Vec<u8>> {
        use crate::control_mapping::{load_default_mappings, MidiSpec};

        // Parse incoming MIDI from app
        let input_msg = match crate::midi::MidiMessage::parse(raw_data) {
            Some(msg) => msg,
            None => return Some(raw_data.to_vec()), // Pass through invalid/sys messages
        };

        // Get normalized value from input
        let normalized_value = match input_msg {
            crate::midi::MidiMessage::PitchBend { value, .. } => {
                crate::midi::convert::to_percent_14bit(value) / 100.0
            },
            crate::midi::MidiMessage::ControlChange { value, .. } => {
                crate::midi::convert::to_percent_7bit(value) / 100.0
            },
            crate::midi::MidiMessage::NoteOn { velocity, .. } => {
                crate::midi::convert::to_percent_7bit(velocity) / 100.0
            },
            crate::midi::MidiMessage::NoteOff { .. } => 0.0,
            _ => return Some(raw_data.to_vec()), // Pass through other messages
        };

        // PAGE-AWARE FILTERING: Check if app is mapped on active page BEFORE scheduling setpoints
        // This prevents faders from moving on Page 2 when Voicemeeter sends feedback
        let config = self.config.read().await;
        let active_page_idx = *self.active_page_index.read().await;

        let active_page = match config.pages.get(active_page_idx) {
            Some(page) => page,
            None => {
                trace!("No active page, skipping feedback forward to X-Touch");
                return None;
            }
        };

        let apps_on_page = self.get_apps_for_page(active_page, &config);
        if !apps_on_page.contains(app_name) {
            trace!(
                "App '{}' not mapped on active page '{}', skipping X-Touch forward",
                app_name,
                active_page.name
            );
            return None;
        }

        debug!(
            "✓ App '{}' is mapped on page '{}', forwarding feedback to X-Touch",
            app_name,
            active_page.name
        );

        // CRITICAL: Schedule motor setpoints AFTER page filtering
        // Only schedule if the app is actually on this page (prevents off-page movements)
        if let crate::midi::MidiMessage::PitchBend { channel, value } = input_msg {
            let channel1 = channel + 1; // Convert 0-based to 1-based
            debug!(
                "← Scheduling fader setpoint from {}: ch={} value14={}",
                app_name, channel1, value
            );
            self.fader_setpoint.schedule(channel1, value, None);
        }

        // Helper to check if a mapping matches the incoming message
        let matches_mapping = |mapping: &crate::config::ControlMapping| -> bool {
            if mapping.app != app_name {
                return false;
            }

            if let Some(midi_spec) = &mapping.midi {
                match midi_spec.midi_type {
                    crate::config::MidiType::Cc => {
                        if let (Some(target_ch), Some(target_cc)) =
                            (midi_spec.channel, midi_spec.cc)
                        {
                            if let crate::midi::MidiMessage::ControlChange { channel, cc, .. } =
                                input_msg
                            {
                                return channel == target_ch.saturating_sub(1) && cc == target_cc;
                            }
                        }
                    },
                    crate::config::MidiType::Note => {
                        if let (Some(target_ch), Some(target_note)) =
                            (midi_spec.channel, midi_spec.note)
                        {
                            match input_msg {
                                crate::midi::MidiMessage::NoteOn { channel, note, .. }
                                | crate::midi::MidiMessage::NoteOff { channel, note, .. } => {
                                    return channel == target_ch.saturating_sub(1)
                                        && note == target_note;
                                },
                                _ => {},
                            }
                        }
                    },
                    crate::config::MidiType::Pb => {
                        if let Some(target_ch) = midi_spec.channel {
                            if let crate::midi::MidiMessage::PitchBend { channel, .. } = input_msg {
                                return channel == target_ch.saturating_sub(1);
                            }
                        }
                    },
                    _ => {},
                }
            }
            false
        };

        // Search in active page controls (use the active_page we already have)
        let mut found_control_id = None;

        if let Some(controls) = &active_page.controls {
            for (id, mapping) in controls {
                if matches_mapping(mapping) {
                    found_control_id = Some(id.clone());
                    break;
                }
            }
        }

        // If not found, search in global controls
        if found_control_id.is_none() {
            if let Some(global) = &config.pages_global {
                if let Some(controls) = &global.controls {
                    for (id, mapping) in controls {
                        if matches_mapping(mapping) {
                            found_control_id = Some(id.clone());
                            break;
                        }
                    }
                }
            }
        }

        // Also check X-Touch mode
        let is_mcu_mode = config
            .xtouch
            .as_ref()
            .map(|x| matches!(x.mode, crate::config::XTouchMode::Mcu))
            .unwrap_or(true);

        drop(config);

        if let Some(control_id) = found_control_id {
            // Load hardware mapping to find native message
            if let Ok(db) = load_default_mappings() {
                if let Some(native_spec) = db.get_midi_spec(&control_id, is_mcu_mode) {
                    // Construct native message with scaled value
                    let native_msg = match native_spec {
                        MidiSpec::ControlChange { cc } => {
                            // For X-Touch, CCs are usually on channel 1 (0)
                            // But we should probably check the group or assume standard MCU
                            Some(crate::midi::MidiMessage::ControlChange {
                                channel: 0, // Default to Ch 1 for buttons
                                cc,
                                value: crate::midi::convert::from_percent_7bit(
                                    normalized_value * 100.0,
                                ),
                            })
                        },
                        MidiSpec::Note { note } => {
                            let velocity =
                                crate::midi::convert::from_percent_7bit(normalized_value * 100.0);
                            if velocity == 0 {
                                Some(crate::midi::MidiMessage::NoteOff {
                                    channel: 0, // Default to Ch 1 for buttons
                                    note,
                                    velocity: 0,
                                })
                            } else {
                                Some(crate::midi::MidiMessage::NoteOn {
                                    channel: 0, // Default to Ch 1 for buttons
                                    note,
                                    velocity,
                                })
                            }
                        },
                        MidiSpec::PitchBend { channel } => {
                            let value14 =
                                crate::midi::convert::from_percent_14bit(normalized_value * 100.0);

                            // CRITICAL: Schedule motor setpoint for CC→PB transformations
                            // This handles the case where QLC+ sends CC but the fader needs PB
                            let channel1 = channel + 1; // Convert 0-based to 1-based
                            debug!(
                                "← Scheduling fader setpoint (CC→PB): {} -> ch={} value14={}",
                                app_name, channel1, value14
                            );
                            self.fader_setpoint.schedule(channel1, value14, None);

                            Some(crate::midi::MidiMessage::PitchBend {
                                channel,
                                value: value14,
                            })
                        },
                    };

                    if let Some(msg) = native_msg {
                        debug!(
                            "← Feedback Transform: {} -> {} ({} -> {})",
                            app_name, control_id, input_msg, msg
                        );
                        return Some(msg.to_bytes());
                    }
                }
            }
        }

        // No mapping found, pass through raw
        Some(raw_data.to_vec())
    }

    /// Refresh the active page (replay all known states to X-Touch)
    pub async fn refresh_page(&self) {
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
            entries
                .iter()
                .filter(|e| e.addr.status == MidiStatus::Note)
                .count(),
            entries
                .iter()
                .filter(|e| e.addr.status == MidiStatus::CC)
                .count(),
            entries
                .iter()
                .filter(|e| e.addr.status == MidiStatus::PB)
                .count()
        );

        // Log PB entries in detail
        for entry in entries.iter().filter(|e| e.addr.status == MidiStatus::PB) {
            debug!(
                "  PB entry: ch={} value={:?} port={}",
                entry.addr.channel.unwrap_or(0),
                entry.value,
                entry.addr.port_id
            );
        }

        // Convert entries to MIDI bytes and collect for sending
        let mut midi_messages = Vec::new();
        for entry in &entries {
            let bytes = self.entry_to_midi_bytes(entry);
            if !bytes.is_empty() {
                if entry.addr.status == MidiStatus::PB {
                    debug!(
                        "  Converting PB entry to MIDI: ch={} value={:?} → bytes={:02X?}",
                        entry.addr.channel.unwrap_or(0),
                        entry.value,
                        bytes
                    );
                }
                midi_messages.push(bytes);
            }
        }

        info!(
            "Page refresh completed: {} (sending {} MIDI messages)",
            page.name,
            midi_messages.len()
        );

        // Store pending MIDI messages for main loop to send to X-Touch
        *self.pending_midi_messages.lock().await = midi_messages;

        // Signal that display needs update (LCD + LEDs)
        *self.display_needs_update.lock().await = true;
    }

    /// Clear X-Touch shadow state (allows re-emission during refresh)
    fn clear_xtouch_shadow(&self) {
        // X-Touch shadow is per-app, clear all
        if let Ok(mut shadows) = self.app_shadows.write() {
            shadows.clear();
        }
    }

    /// Convert MidiStateEntry to raw MIDI bytes for sending to X-Touch
    fn entry_to_midi_bytes(&self, entry: &MidiStateEntry) -> Vec<u8> {
        use crate::state::MidiValue;

        // Convert external channel (1-16) to MIDI wire format (0-15)
        let channel = entry.addr.channel.map(|ch| ch.saturating_sub(1)).unwrap_or(0);
        let data1 = entry.addr.data1.unwrap_or(0);

        match entry.addr.status {
            MidiStatus::Note => {
                let velocity = match &entry.value {
                    MidiValue::Number(v) => (*v as u8).min(127),
                    _ => 0,
                };
                // Always use Note On - velocity 0 turns LED off, velocity >0 turns it on
                // NoteOff (0x80) is for button release events, not LED control
                vec![0x90 | channel, data1, velocity]
            }
            MidiStatus::CC => {
                let value = match &entry.value {
                    MidiValue::Number(v) => (*v as u8).min(127),
                    _ => 0,
                };
                vec![0xB0 | channel, data1, value] // Control Change
            }
            MidiStatus::PB => {
                let value14 = match &entry.value {
                    MidiValue::Number(v) => (*v).min(16383),
                    _ => 0,
                };
                let lsb = (value14 & 0x7F) as u8;
                let msb = ((value14 >> 7) & 0x7F) as u8;
                vec![0xE0 | channel, lsb, msb] // Pitch Bend
            }
            _ => vec![], // Other types not handled
        }
    }

    /// Try to transform CC value to PB for page refresh (reverse transformation)
    ///
    /// When a fader is mapped to send CC to an app (like QLC+), the StateStore
    /// will have CC values. But X-Touch faders need PB messages. This function:
    /// 1. Looks up which control uses the given PB channel
    /// 2. Checks if that control has a CC mapping in the page config
    /// 3. Queries StateStore for the CC value
    /// 4. Transforms CC (7-bit) to PB (14-bit) using fast approximation
    ///
    /// Returns transformed PB entry if CC value found, None otherwise
    fn try_cc_to_pb_transform(
        &self,
        page: &PageConfig,
        app: &AppKey,
        pb_channel: u8,
    ) -> Option<MidiStateEntry> {
        use crate::control_mapping::{load_default_mappings, MidiSpec};
        use crate::state::{MidiAddr, MidiValue, Origin};

        debug!(
            "🔄 CC→PB transform: app={:?} page={} pb_channel={}",
            app, page.name, pb_channel
        );

        // 1. Reverse lookup: Find control ID for this PB channel (e.g., "fader1" for ch1)
        let mapping_db = match load_default_mappings() {
            Ok(db) => db,
            Err(e) => {
                debug!("❌ Failed to load mapping DB: {}", e);
                return None;
            }
        };

        let control_id = mapping_db
            .mappings
            .iter()
            .find(|(_, mapping)| {
                if let Ok(spec) = MidiSpec::parse(&mapping.mcu_message) {
                    matches!(spec, MidiSpec::PitchBend { channel } if channel == pb_channel.saturating_sub(1))
                } else {
                    false
                }
            })
            .map(|(id, _)| id.clone());

        let control_id = match control_id {
            Some(id) => {
                debug!("  ✓ Found control_id: {}", id);
                id
            }
            None => {
                debug!("  ❌ No control found for PB channel {}", pb_channel);
                return None;
            }
        };

        // 2. Get control config from page
        let page_controls = match page.controls.as_ref() {
            Some(controls) => controls,
            None => {
                debug!("  ❌ Page has no controls");
                return None;
            }
        };

        let control_config = match page_controls.get(&control_id) {
            Some(config) => {
                debug!("  ✓ Found control config for {}: app={}", control_id, config.app);
                config
            }
            None => {
                debug!("  ❌ Control '{}' not found in page", control_id);
                return None;
            }
        };

        // Ensure control's app matches the app we're querying for
        if control_config.app != app.as_str() {
            debug!("  ❌ Control app '{}' doesn't match queried app '{:?}'", control_config.app, app);
            return None;
        }

        // 3. Check if control has CC mapping (not PB passthrough)
        let midi_spec = match control_config.midi.as_ref() {
            Some(spec) => spec,
            None => {
                debug!("  ❌ Control has no MIDI spec");
                return None;
            }
        };

        if !matches!(midi_spec.midi_type, crate::config::MidiType::Cc) {
            debug!("  ❌ Control MIDI type is not CC (is {:?})", midi_spec.midi_type);
            return None;
        }

        // 4. Query StateStore for CC value
        let cc_channel = match midi_spec.channel {
            Some(ch) => ch,
            None => {
                debug!("  ❌ CC spec has no channel");
                return None;
            }
        };
        let cc_num = match midi_spec.cc {
            Some(num) => num,
            None => {
                debug!("  ❌ CC spec has no cc number");
                return None;
            }
        };

        debug!(
            "  → Querying StateStore: app={:?} CC ch={} cc={}",
            app, cc_channel, cc_num
        );

        let cc_entry = match self.state.get_known_latest_for_app(
            *app,
            crate::state::MidiStatus::CC,
            Some(cc_channel),
            Some(cc_num),
        ) {
            Some(entry) => {
                debug!("  ✓ Found CC entry: value={:?}", entry.value);
                entry
            }
            None => {
                debug!("  ❌ No CC entry found in StateStore");
                return None;
            }
        };

        // 5. Transform CC (7-bit) to PB (14-bit)
        // Use TypeScript fast approximation: (v7 << 7) | v7
        let cc_value = match cc_entry.value.as_number() {
            Some(num) => num as u8,
            None => {
                debug!("  ❌ CC value is not a number");
                return None;
            }
        };
        let pb_value = ((cc_value as u16) << 7) | (cc_value as u16);

        debug!(
            "  ✓ Transform CC {} → PB {} (0x{:04X})",
            cc_value, pb_value, pb_value
        );

        // 6. Create transformed PB entry
        Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: app.as_str().to_string(),
                status: crate::state::MidiStatus::PB,
                channel: Some(pb_channel),
                data1: Some(0),
            },
            value: MidiValue::Number(pb_value),
            ts: cc_entry.ts,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        })
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
            let key = format!(
                "{}|{}",
                e.addr.channel.unwrap_or(0),
                e.addr.data1.unwrap_or(0)
            );
            let should_insert = match map.get(&key) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(
                    key,
                    PlanEntry {
                        entry: e,
                        priority: prio,
                    },
                );
            }
        };

        let push_cc = |map: &mut HashMap<String, PlanEntry>, e: MidiStateEntry, prio: u8| {
            let key = format!(
                "{}|{}",
                e.addr.channel.unwrap_or(0),
                e.addr.data1.unwrap_or(0)
            );
            let should_insert = match map.get(&key) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(
                    key,
                    PlanEntry {
                        entry: e,
                        priority: prio,
                    },
                );
            }
        };

        let push_pb = |map: &mut HashMap<u8, PlanEntry>, ch: u8, e: MidiStateEntry, prio: u8| {
            let should_insert = match map.get(&ch) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(
                    ch,
                    PlanEntry {
                        entry: e,
                        priority: prio,
                    },
                );
            }
        };

        // X-Touch buttons are on channel 1 (0-indexed as channel 0 in MIDI, but config uses 1)
        // Faders use channels 1-9 for PitchBend: 8 strip faders + 1 master fader
        // (fader1-8 on ch1-8, fader_master on ch9)
        let channels: Vec<u8> = (1..=9).collect();

        // Get apps mapped on this page (only restore state for mapped apps)
        let config = self.config.try_read().expect("Config lock poisoned");
        let apps_on_page = self.get_apps_for_page(_page, &config);

        // Build plans for each app (only apps mapped on this page)
        for app in AppKey::all() {
            // Skip apps not mapped on this page
            if !apps_on_page.contains(app.as_str()) {
                continue;
            }
            // PB plan (priority: Known PB = 3 > Mapped CC→PB = 2 > Fader Setpoint = 2 > Zero = 1)
            for &ch in &channels {
                // Priority 3: Try to get known PB value
                if let Some(latest_pb) =
                    self.state
                        .get_known_latest_for_app(*app, MidiStatus::PB, Some(ch), Some(0))
                {
                    push_pb(&mut pb_plan, ch, latest_pb, 3);
                    continue;
                }

                // Priority 2: Try to transform CC to PB (for apps like QLC+)
                if let Some(transformed_pb) = self.try_cc_to_pb_transform(_page, app, ch) {
                    debug!("  ✅ Adding CC→PB to plan: ch={} value={:?}", ch, transformed_pb.value);
                    push_pb(&mut pb_plan, ch, transformed_pb, 2);
                    continue;
                }

                // Priority 2: Try to get from fader setpoint (motor position)
                if let Some(desired14) = self.fader_setpoint.get_desired(ch) {
                    let setpoint_pb = MidiStateEntry {
                        addr: MidiAddr {
                            port_id: app.as_str().to_string(),
                            status: MidiStatus::PB,
                            channel: Some(ch),
                            data1: Some(0),
                        },
                        value: MidiValue::Number(desired14),
                        ts: Self::now_ms(),
                        origin: Origin::XTouch,
                        known: false,
                        stale: false,
                        hash: None,
                    };
                    push_pb(&mut pb_plan, ch, setpoint_pb, 2);
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

            // Notes: 0-31 - Always send Note Off to clear previous page buttons
            // Then let drivers send feedback to turn on buttons that should be ON
            for &ch in &channels {
                for note in 0..=31 {
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

            // CC (rings): 0-31 - Always send 0 to clear previous page
            for &ch in &channels {
                for cc in 0..=31 {
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

    /// Get current timestamp in milliseconds
    fn now_ms() -> u64 {
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

    /// Get anti-echo window for a MIDI status type
    fn get_anti_echo_window(status: MidiStatus) -> u64 {
        ANTI_ECHO_WINDOWS
            .iter()
            .find(|(s, _)| *s == status)
            .map(|(_, ms)| *ms)
            .unwrap_or(60)
    }

    /// Update F1-F8 LEDs to reflect active page
    ///
    /// Matches TypeScript updateFKeyLedsForActivePage() from xtouch/fkeys.ts
    pub async fn update_fkey_leds_for_active_page(
        &self,
        xtouch: &crate::xtouch::XTouchDriver,
        _paging_channel: u8,
    ) -> Result<()> {
        let config = self.config.read().await;
        let active_index = *self.active_page_index.read().await;

        // Get F-key notes based on mode
        let mode = config
            .xtouch
            .as_ref()
            .map(|x| x.mode)
            .unwrap_or(crate::config::XTouchMode::Mcu);
        let fkey_notes = self.get_fkey_notes(mode);

        // Clamp active index to valid range
        let clamped_index = if active_index < fkey_notes.len() {
            active_index as i32
        } else {
            (fkey_notes.len().saturating_sub(1)) as i32
        };

        // Update LEDs - ALWAYS turn all off first, then light the active one
        for (i, &note) in fkey_notes.iter().enumerate() {
            let on = (i as i32) == clamped_index;
            xtouch.set_button_led(note, on).await?;
        }

        debug!(
            "F-key LEDs updated: active index {} (note {})",
            clamped_index,
            fkey_notes.get(clamped_index as usize).copied().unwrap_or(0)
        );

        Ok(())
    }

    /// Update prev/next navigation button LEDs (always on)
    ///
    /// Matches TypeScript updatePrevNextLeds() from xtouch/fkeys.ts
    pub async fn update_prev_next_leds(
        &self,
        xtouch: &crate::xtouch::XTouchDriver,
        prev_note: u8,
        next_note: u8,
    ) -> Result<()> {
        xtouch.set_button_led(prev_note, true).await?;
        xtouch.set_button_led(next_note, true).await?;
        Ok(())
    }

    /// Get F-key note numbers based on X-Touch mode
    fn get_fkey_notes(&self, mode: crate::config::XTouchMode) -> Vec<u8> {
        // From xtouch-matching.csv for MCU mode:
        // f1 = 54, f2 = 55, f3 = 56, f4 = 57, f5 = 58, f6 = 59, f7 = 60, f8 = 61
        // These are the default note numbers for F1-F8 in both MCU and Ctrl modes
        match mode {
            crate::config::XTouchMode::Mcu | crate::config::XTouchMode::Ctrl => {
                vec![54, 55, 56, 57, 58, 59, 60, 61]
            },
        }
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
        let app_shadow = app_shadows
            .entry(app_key.to_string())
            .or_insert_with(HashMap::new);
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

        let last_user_ts = self
            .last_user_action_ts
            .try_read()
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
            },
            0xB => {
                // Control Change
                let cc = raw.get(1).copied().unwrap_or(0);
                format!("cc|{}|{}", channel, cc)
            },
            0xE => {
                // Pitch Bend
                format!("pb|{}|0", channel)
            },
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
            },
        };

        // Update state store (this also notifies subscribers)
        let app = match AppKey::from_str(app_key) {
            Some(a) => a,
            None => {
                warn!("Unknown application key: {}", app_key);
                return;
            },
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

    /// Evaluate indicator conditions for a signal emission
    ///
    /// Returns a HashMap of control_id -> should_be_lit for all controls
    /// on the active page that have indicators matching the given signal.
    ///
    /// This is called by the indicator subscription handler when drivers
    /// emit signals (e.g., "obs.selectedScene", "obs.studioMode").
    pub async fn evaluate_indicators(&self, signal: &str, value: &Value) -> HashMap<String, bool> {
        let mut result = HashMap::new();

        // Get active page controls
        let config = self.config.read().await;
        let page_index = *self.active_page_index.read().await;

        let page = match config.pages.get(page_index) {
            Some(p) => p,
            None => return result,
        };

        let controls = match &page.controls {
            Some(c) => c,
            None => return result,
        };

        // Also check global controls
        let global_controls = config.pages_global.as_ref().and_then(|g| g.controls.as_ref());

        // Iterate through all controls (page + global)
        // First check page controls
        for (control_id, mapping) in controls.iter() {
            let indicator = match &mapping.indicator {
                Some(ind) => ind,
                None => continue,
            };

            // Check if this indicator matches the signal
            if indicator.signal != signal {
                continue;
            }

            // Evaluate the condition
            let should_be_lit = if let Some(truthy) = indicator.truthy {
                // Truthy check: LED on if value is truthy
                if truthy {
                    match value {
                        Value::Bool(b) => *b,
                        Value::Null => false,
                        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
                        Value::String(s) => !s.is_empty(),
                        Value::Array(a) => !a.is_empty(),
                        Value::Object(o) => !o.is_empty(),
                    }
                } else {
                    false
                }
            } else if let Some(in_array) = &indicator.in_array {
                // "in" check: LED on if value matches any in array
                in_array.iter().any(|v| {
                    // String comparison: trim and compare
                    if let (Value::String(a), Value::String(b)) = (v, value) {
                        a.trim() == b.trim()
                    } else {
                        // Use serde_json equality (similar to Object.is)
                        v == value
                    }
                })
            } else if let Some(equals_value) = &indicator.equals {
                // Equals check: LED on if value matches exactly
                if let (Value::String(a), Value::String(b)) = (equals_value, value) {
                    a.trim() == b.trim()
                } else {
                    equals_value == value
                }
            } else {
                // No condition specified, default to off
                false
            };

            result.insert(control_id.clone(), should_be_lit);
        }

        // Then check global controls
        if let Some(global_ctrls) = global_controls {
            for (control_id, mapping) in global_ctrls.iter() {
                let indicator = match &mapping.indicator {
                    Some(ind) => ind,
                    None => continue,
                };

                // Check if this indicator matches the signal
                if indicator.signal != signal {
                    continue;
                }

                // Evaluate the condition
                let should_be_lit = if let Some(truthy) = indicator.truthy {
                    // Truthy check: LED on if value is truthy
                    if truthy {
                        match value {
                            Value::Bool(b) => *b,
                            Value::Null => false,
                            Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
                            Value::String(s) => !s.is_empty(),
                            Value::Array(a) => !a.is_empty(),
                            Value::Object(o) => !o.is_empty(),
                        }
                    } else {
                        false
                    }
                } else if let Some(in_array) = &indicator.in_array {
                    // "in" check: LED on if value matches any in array
                    in_array.iter().any(|v| {
                        // String comparison: trim and compare
                        if let (Value::String(a), Value::String(b)) = (v, value) {
                            a.trim() == b.trim()
                        } else {
                            // Use serde_json equality (similar to Object.is)
                            v == value
                        }
                    })
                } else if let Some(equals_value) = &indicator.equals {
                    // Equals check: LED on if value matches exactly
                    if let (Value::String(a), Value::String(b)) = (equals_value, value) {
                        a.trim() == b.trim()
                    } else {
                        equals_value == value
                    }
                } else {
                    // No condition specified, default to off
                    false
                };

                result.insert(control_id.clone(), should_be_lit);
            }
        }

        result
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
        let config = make_test_config(vec![make_test_page("Voicemeeter"), make_test_page("OBS")]);

        let router = Router::new(config);

        router.set_active_page("OBS").await.unwrap();
        assert_eq!(router.get_active_page_name().await, "OBS");

        router.set_active_page("voicemeeter").await.unwrap(); // Case insensitive
        assert_eq!(router.get_active_page_name().await, "Voicemeeter");
    }

    #[tokio::test]
    async fn test_set_page_by_index() {
        let config = make_test_config(vec![make_test_page("Page 0"), make_test_page("Page 1")]);

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
        let config = make_test_config(vec![make_test_page("Page 1"), make_test_page("Page 2")]);

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
        let result = router
            .register_driver("test_console".to_string(), driver)
            .await;

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
            .register_driver(
                "driver1".to_string(),
                Arc::new(ConsoleDriver::new("driver1")),
            )
            .await
            .unwrap();

        router
            .register_driver(
                "driver2".to_string(),
                Arc::new(ConsoleDriver::new("driver2")),
            )
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

        let initial_config =
            make_test_config(vec![make_test_page("Page 1"), make_test_page("Page 2")]);

        let router = Router::new(initial_config);

        // Register a driver
        router
            .register_driver(
                "test_driver".to_string(),
                Arc::new(ConsoleDriver::new("test_driver")),
            )
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
            .register_driver(
                "test_console".to_string(),
                Arc::new(ConsoleDriver::new("test_console")),
            )
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Driver 'missing_driver' not registered"));
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
            .register_driver(
                "test_console".to_string(),
                Arc::new(ConsoleDriver::new("test_console")),
            )
            .await
            .unwrap();

        // Attempt to execute non-existent control
        let result = router
            .handle_control("non_existent_control", Some(json!(127)))
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No mapping for control"));
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
            .register_driver(
                "obs_driver".to_string(),
                Arc::new(ConsoleDriver::new("obs_driver")),
            )
            .await
            .unwrap();

        router
            .register_driver(
                "vm_driver".to_string(),
                Arc::new(ConsoleDriver::new("vm_driver")),
            )
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
