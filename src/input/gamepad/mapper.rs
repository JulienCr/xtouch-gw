//! Gamepad event mapper - transforms gamepad events to router commands

use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};
use serde_json::Value;

use crate::config::{GamepadConfig, AnalogConfig};
use crate::router::Router;
use super::provider::GamepadEvent;
use super::hybrid_provider::{HybridGamepadProvider, EventCallback};
use super::analog::{process_axis, apply_inversion};

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
    ///
    /// # Returns
    /// Configured mapper instance
    pub async fn attach(
        provider: Arc<HybridGamepadProvider>,
        router: Arc<Router>,
        _config: &GamepadConfig,
    ) -> Result<Self> {
        // Create channel for sequential event processing
        let (event_tx, event_rx) = mpsc::unbounded_channel::<GamepadEvent>();

        // Spawn single task that processes events SEQUENTIALLY
        // This guarantees order and eliminates race conditions
        Self::spawn_event_processor(event_rx, router.clone());

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
    ) {
        // Cache for redundant event filtering (no sequence needed - we process in order!)
        let mut last_axis_values: HashMap<String, f32> = HashMap::new();

        tokio::spawn(async move {
            debug!("Gamepad event processor started (sequential mode)");

            while let Some(event) = event_rx.recv().await {
                match event {
                    GamepadEvent::Button { control_id, pressed } => {
                        if let Err(e) = Self::handle_button(&control_id, pressed, &router).await {
                            error!("Error handling button event: {}", e);
                        }
                    }
                    GamepadEvent::Axis { control_id, value, analog_config, sequence: _ } => {
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
    ) -> Result<()> {
        debug!("Button event: {} = {}", control_id, pressed);

        // Only trigger on press (not release)
        if !pressed {
            return Ok(());
        }

        // Send to router (router will look up mapping internally)
        match router.handle_control(control_id, None).await {
            Ok(_) => {
                debug!("✅ Router handled control: {}", control_id);
            }
            Err(e) => {
                debug!("⚠️  Router error for {}: {}", control_id, e);
            }
        }

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
                    match router.handle_control(control_id, Some(Value::from(0.0))).await {
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
            match router.handle_control(control_id, Some(Value::from(final_value))).await {
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
