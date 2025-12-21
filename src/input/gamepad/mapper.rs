//! Gamepad event mapper - transforms gamepad events to router commands

#![allow(dead_code)]

use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{debug, error};
use serde_json::Value;

use crate::config::{GamepadConfig, AnalogConfig};
use crate::router::Router;
use super::provider::GamepadEvent;
use super::hybrid_provider::{HybridGamepadProvider, EventCallback};
use super::analog::{process_axis, apply_inversion};

/// Gamepad mapper - connects provider events to router
pub struct GamepadMapper {
    _provider: Arc<HybridGamepadProvider>,
    router: Arc<Router>,
    analog_config: Option<AnalogConfig>,
    last_axis_values: Arc<std::sync::RwLock<HashMap<String, f32>>>,
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
        config: &GamepadConfig,
    ) -> Result<Self> {
        let analog_config = config.analog.clone();
        let last_axis_values = Arc::new(std::sync::RwLock::new(HashMap::new()));

        // Subscribe to provider events
        let router_clone = router.clone();
        let cache_clone = last_axis_values.clone();

        let callback: EventCallback = Arc::new(move |event| {
            let router = router_clone.clone();
            let cache = cache_clone.clone();

            // Spawn async task to handle event
            // Note: analog_config is now embedded in Axis events
            tokio::spawn(async move {
                if let Err(e) = Self::handle_event(event, &router, &cache).await {
                    error!("Error handling gamepad event: {}", e);
                }
            });
        });

        provider.subscribe(callback).await;

        Ok(Self {
            _provider: provider,
            router,
            analog_config,
            last_axis_values,
        })
    }

    /// Handle a single gamepad event
    async fn handle_event(
        event: GamepadEvent,
        router: &Arc<Router>,
        cache: &Arc<std::sync::RwLock<HashMap<String, f32>>>,
    ) -> Result<()> {
        match event {
            GamepadEvent::Button { control_id, pressed } => {
                Self::handle_button(control_id, pressed, router).await?;
            }
            GamepadEvent::Axis { control_id, value, analog_config } => {
                Self::handle_axis(control_id, value, router, &analog_config, cache).await?;
            }
        }

        Ok(())
    }

    /// Handle button event
    async fn handle_button(
        control_id: String,
        pressed: bool,
        router: &Arc<Router>,
    ) -> Result<()> {
        debug!("Button event: {} = {}", control_id, pressed);

        // Only trigger on press (not release)
        if !pressed {
            return Ok(());
        }

        // Send to router (router will look up mapping internally)
        match router.handle_control(&control_id, None).await {
            Ok(_) => {
                debug!("✅ Router handled control: {}", control_id);
            }
            Err(e) => {
                debug!("⚠️  Router error for {}: {}", control_id, e);
            }
        }

        Ok(())
    }

    /// Handle axis event
    async fn handle_axis(
        control_id: String,
        raw_value: f32,
        router: &Arc<Router>,
        analog_config: &Option<AnalogConfig>,
        cache: &Arc<std::sync::RwLock<HashMap<String, f32>>>,
    ) -> Result<()> {
        // Extract axis name (e.g., "lx" from "gamepad1.axis.lx" or "gamepad.axis.lx")
        let axis_name = control_id
            .rsplit_once(".axis.")
            .and_then(|(_, name)| Some(name))
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
                // Within deadzone (gilrs only) - check cache before sending 0.0
                let should_send_zero = {
                    let cache_read = cache.read().unwrap();
                    cache_read.get(&control_id).map_or(true, |&last| last != 0.0)
                };

                if should_send_zero {
                    debug!("Axis event: {} = 0.0 (deadzone)", control_id);
                    match router.handle_control(&control_id, Some(Value::from(0.0))).await {
                        Ok(_) => debug!("✅ Router handled axis (deadzone): {} = 0.0", control_id),
                        Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
                    }
                    // Update cache
                    cache.write().unwrap().insert(control_id, 0.0);
                } else {
                    debug!("Axis already at 0.0, skipping redundant event: {}", control_id);
                }
                return Ok(());
            }
        };

        // Check cache to avoid sending redundant values
        let should_send = {
            let cache_read = cache.read().unwrap();
            cache_read.get(&control_id).map_or(true, |&last| last != final_value)
        };

        if should_send {
            debug!("Axis event: {} = {:.3} (raw: {:.3})", control_id, final_value, raw_value);

            // Send to router (pass normalized value, driver will handle scaling)
            match router.handle_control(&control_id, Some(Value::from(final_value))).await {
                Ok(_) => debug!("✅ Router handled axis: {} = {:.3}", control_id, final_value),
                Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
            }

            // Update cache
            cache.write().unwrap().insert(control_id, final_value);
        } else {
            debug!("Axis value unchanged ({}), skipping redundant event: {}", final_value, control_id);
        }

        Ok(())
    }

    /// Shutdown the mapper
    pub async fn shutdown(&self) -> Result<()> {
        // Provider will be dropped automatically
        Ok(())
    }
}
