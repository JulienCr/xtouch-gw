//! Gamepad event mapper - transforms gamepad events to router commands

use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};
use serde_json::Value;

use crate::api::CameraStateMessage;
use crate::config::{GamepadConfig, AnalogConfig};
use crate::router::Router;

use super::provider::GamepadEvent;
use super::hybrid_provider::{HybridGamepadProvider, EventCallback};
use super::analog::{process_axis, apply_inversion};

/// LT threshold for "pressed" detection (normalized value -1.0 to 1.0)
/// LT is normalized from 0-255 to -1.0..1.0, so 0.3 means ~65% pressed
const LT_THRESHOLD: f32 = 0.3;

/// Gamepad mapper - connects provider events to router
///
/// Uses a dedicated channel + single task for SEQUENTIAL event processing.
/// This eliminates race conditions that occur when spawning tasks per event.
pub struct GamepadMapper {
    _provider: Arc<HybridGamepadProvider>,
    /// Channel sender kept alive to prevent task shutdown
    _event_tx: mpsc::UnboundedSender<GamepadEvent>,
}

impl GamepadMapper {
    /// Create and attach a gamepad mapper
    ///
    /// # Arguments
    /// * `provider` - Hybrid gamepad provider instance
    /// * `router` - Router instance
    /// * `config` - Gamepad configuration
    /// * `update_tx` - Broadcast sender for camera state updates (Stream Deck notifications)
    ///
    /// # Returns
    /// Configured mapper instance
    pub async fn attach(
        provider: Arc<HybridGamepadProvider>,
        router: Arc<Router>,
        _config: &GamepadConfig,
        update_tx: broadcast::Sender<CameraStateMessage>,
    ) -> Result<Self> {
        // Create channel for sequential event processing
        let (event_tx, event_rx) = mpsc::unbounded_channel::<GamepadEvent>();

        // Spawn single task that processes events SEQUENTIALLY
        // This guarantees order and eliminates race conditions
        Self::spawn_event_processor(event_rx, router.clone(), update_tx);

        // Subscribe to provider events - just forward to channel
        let tx_clone = event_tx.clone();
        let callback: EventCallback = Arc::new(move |event| {
            if let Err(e) = tx_clone.send(event) {
                warn!("Failed to send gamepad event to processor: {}", e);
            }
        });

        provider.subscribe(callback).await;

        Ok(Self {
            _provider: provider,
            _event_tx: event_tx,
        })
    }

    /// Spawn the sequential event processor task
    fn spawn_event_processor(
        mut event_rx: mpsc::UnboundedReceiver<GamepadEvent>,
        router: Arc<Router>,
        update_tx: broadcast::Sender<CameraStateMessage>,
    ) {
        // Cache for redundant event filtering (no sequence needed - we process in order!)
        let mut last_axis_values: HashMap<String, f32> = HashMap::new();
        // Track LT (left trigger) held state per gamepad for PTZ modifier
        let mut lt_held: HashMap<String, bool> = HashMap::new();

        tokio::spawn(async move {
            debug!("Gamepad event processor started (sequential mode)");

            while let Some(event) = event_rx.recv().await {
                match event {
                    GamepadEvent::Button { control_id, pressed } => {
                        // Track LT (left trigger) button state for PTZ modifier
                        if control_id.ends_with(".btn.lt") {
                            let prefix = control_id.split('.').next().unwrap_or("");
                            lt_held.insert(prefix.to_string(), pressed);
                            debug!("LT modifier: {} = {}", prefix, pressed);
                            // Don't route LT button to router, just track state
                            continue;
                        }

                        if let Err(e) = Self::handle_button(&control_id, pressed, &router, &lt_held, &update_tx).await {
                            error!("Error handling button event: {}", e);
                        }
                    }
                    GamepadEvent::Axis { control_id, value, analog_config, sequence: _ } => {
                        // Also track LT from axis events (some controllers send axis instead of button)
                        if control_id.ends_with(".axis.zl") {
                            let prefix = control_id.split('.').next().unwrap_or("");
                            let is_pressed = value > LT_THRESHOLD;
                            lt_held.insert(prefix.to_string(), is_pressed);
                        }

                        // sequence is ignored - we process in order!
                        if let Err(e) = Self::handle_axis(
                            &control_id,
                            value,
                            &router,
                            &analog_config,
                            &mut last_axis_values,
                        ).await {
                            error!("Error handling axis event: {}", e);
                        }
                    }
                }
            }

            debug!("Gamepad event processor stopped");
        });
    }

    /// Handle button event
    async fn handle_button(
        control_id: &str,
        pressed: bool,
        router: &Arc<Router>,
        lt_held: &HashMap<String, bool>,
        update_tx: &broadcast::Sender<CameraStateMessage>,
    ) -> Result<()> {
        debug!("Button event: {} = {}", control_id, pressed);

        // Only trigger on press (not release)
        if !pressed {
            return Ok(());
        }

        // Extract gamepad prefix (e.g., "gamepad1" from "gamepad1.btn.a")
        let prefix = control_id.split('.').next().unwrap_or("");
        let is_lt_held = lt_held.get(prefix).copied().unwrap_or(false);

        // Check if this is a camera button (A/B/X/Y)
        let is_camera_button = [".btn.a", ".btn.b", ".btn.x", ".btn.y"]
            .iter()
            .any(|suffix| control_id.ends_with(suffix));

        // Determine extra params based on button type and LT state
        let extra_params = if is_camera_button {
            if is_lt_held {
                // LT+button: Preview mode + PTZ target
                if let Err(e) = Self::handle_ptz_target(control_id, prefix, router, update_tx).await {
                    debug!("PTZ target error: {}", e);
                }
                Some(vec![Value::String("preview".to_string())])
            } else {
                // Camera button alone: Direct to program
                Some(vec![Value::String("program".to_string())])
            }
        } else {
            None
        };

        // Route the control event
        match router.handle_control(control_id, None, extra_params).await {
            Ok(_) => debug!("✅ Router handled control: {}", control_id),
            Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
        }

        Ok(())
    }

    /// Handle LT+button: set PTZ camera target
    async fn handle_ptz_target(
        control_id: &str,
        gamepad_slot: &str,
        router: &Arc<Router>,
        update_tx: &broadcast::Sender<CameraStateMessage>,
    ) -> Result<()> {
        // 1. Get the camera ID from the button's mapping in config
        let camera_id: Option<String> = {
            let config = router.config.read().await;

            // Look up in pages_global.controls
            config.pages_global
                .as_ref()
                .and_then(|pg| pg.controls.as_ref())
                .and_then(|controls| controls.get(control_id))
                .filter(|m| m.action.as_deref() == Some("selectCamera"))
                .and_then(|m| m.params.as_ref())
                .and_then(|params| params.first())
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        let camera_id = match camera_id {
            Some(id) => id,
            None => {
                debug!("No selectCamera mapping for {}, ignoring LT+button", control_id);
                return Ok(());
            }
        };

        // 2. Check if PTZ is enabled for this camera
        let ptz_enabled = {
            let config = router.config.read().await;
            config.obs
                .as_ref()
                .and_then(|o| o.camera_control.as_ref())
                .and_then(|cc| cc.cameras.iter().find(|c| c.id == camera_id))
                .map(|c| c.enable_ptz)
                .unwrap_or(false)
        };

        if !ptz_enabled {
            debug!("PTZ disabled for camera {}, ignoring LT+button", camera_id);
            return Ok(());
        }

        // 3. Set the PTZ target
        let camera_targets = router.get_camera_targets();
        camera_targets.set_target(gamepad_slot, &camera_id)?;

        // 4. Broadcast update to Stream Deck
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let message = CameraStateMessage::TargetChanged {
            gamepad_slot: gamepad_slot.to_string(),
            camera_id: camera_id.clone(),
            timestamp,
        };

        // Best-effort broadcast (ignore if no subscribers)
        let _ = update_tx.send(message);

        info!("PTZ target set: {} -> {}", gamepad_slot, camera_id);
        Ok(())
    }

    /// Handle axis event (SEQUENTIAL - no race conditions!)
    async fn handle_axis(
        control_id: &str,
        raw_value: f32,
        router: &Arc<Router>,
        analog_config: &Option<AnalogConfig>,
        cache: &mut HashMap<String, f32>,
    ) -> Result<()> {
        // Extract axis name (e.g., "lx" from "gamepad1.axis.lx" or "gamepad.axis.lx")
        let axis_name = control_id
            .rsplit_once(".axis.")
            .map(|(_, name)| name)
            .ok_or_else(|| anyhow!("Invalid axis control ID: {}", control_id))?;

        // Check if this is XInput (gamepad2) - skip software deadzone for XInput
        let is_xinput = control_id.starts_with("gamepad2");

        // Process the axis value
        let processed = if is_xinput {
            // XInput already has hardware deadzone (7849), skip software deadzone
            // Just apply gamma and inversion if configured
            if let Some(cfg) = analog_config {
                // Apply gamma curve only (no deadzone)
                let sign = raw_value.signum();
                let magnitude = raw_value.abs();
                let curved = magnitude.powf(cfg.gamma);
                Some(sign * curved)
            } else {
                // No config, use raw value
                Some(raw_value)
            }
        } else {
            // gilrs (gamepad1/FaceOff) needs software deadzone processing
            match analog_config {
                Some(cfg) => process_axis(raw_value, cfg),
                None => Some(raw_value),
            }
        };

        // Get final value after processing
        let final_value = match processed {
            Some(v) => {
                // Apply inversion if configured
                if let Some(cfg) = analog_config {
                    apply_inversion(v, axis_name, cfg)
                } else {
                    v
                }
            }
            None => {
                // Within deadzone (gilrs only) - send 0.0 if last value was non-zero
                let last = cache.get(control_id).copied();
                if last.map_or(true, |v| v != 0.0) {
                    debug!("Axis event: {} = 0.0 (deadzone)", control_id);
                    match router.handle_control(control_id, Some(Value::from(0.0)), None).await {
                        Ok(_) => debug!("✅ Router handled axis (deadzone): {} = 0.0", control_id),
                        Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
                    }
                    cache.insert(control_id.to_string(), 0.0);
                }
                return Ok(());
            }
        };

        // Check cache to avoid sending redundant values
        let last = cache.get(control_id).copied();
        if last.map_or(true, |v| v != final_value) {
            debug!("Axis event: {} = {:.3} (raw: {:.3})", control_id, final_value, raw_value);

            // Send to router (pass normalized value, driver will handle scaling)
            match router.handle_control(control_id, Some(Value::from(final_value)), None).await {
                Ok(_) => debug!("✅ Router handled axis: {} = {:.3}", control_id, final_value),
                Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
            }

            // Update cache
            cache.insert(control_id.to_string(), final_value);
        }

        Ok(())
    }

    /// Shutdown the mapper
    pub async fn shutdown(&self) -> Result<()> {
        // Provider will be dropped automatically
        // Event processor will stop when channel is dropped
        Ok(())
    }
}
