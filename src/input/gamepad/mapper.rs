//! Gamepad event mapper - transforms gamepad events to router commands

use anyhow::{Result, anyhow};
use std::sync::Arc;
use tracing::{warn, debug, error};
use serde_json::Value;

use crate::config::{GamepadConfig, AnalogConfig};
use crate::router::Router;
use super::provider::{GamepadEvent, GilrsProvider, EventCallback};
use super::analog::{process_axis, apply_inversion};

/// Gamepad mapper - connects provider events to router
pub struct GamepadMapper {
    _provider: Arc<GilrsProvider>,
    router: Arc<Router>,
    analog_config: Option<AnalogConfig>,
}

impl GamepadMapper {
    /// Create and attach a gamepad mapper
    ///
    /// # Arguments
    /// * `provider` - Gamepad provider instance
    /// * `router` - Router instance
    /// * `config` - Gamepad configuration
    ///
    /// # Returns
    /// Configured mapper instance
    pub async fn attach(
        provider: Arc<GilrsProvider>,
        router: Arc<Router>,
        config: &GamepadConfig,
    ) -> Result<Self> {
        let analog_config = config.analog.clone();

        // Subscribe to provider events
        let router_clone = router.clone();
        let analog_clone = analog_config.clone();

        let callback: EventCallback = Arc::new(move |event| {
            let router = router_clone.clone();
            let analog = analog_clone.clone();

            // Spawn async task to handle event
            tokio::spawn(async move {
                if let Err(e) = Self::handle_event(event, &router, &analog).await {
                    error!("Error handling gamepad event: {}", e);
                }
            });
        });

        provider.subscribe(callback).await;

        Ok(Self {
            _provider: provider,
            router,
            analog_config,
        })
    }

    /// Handle a single gamepad event
    async fn handle_event(
        event: GamepadEvent,
        router: &Arc<Router>,
        analog_config: &Option<AnalogConfig>,
    ) -> Result<()> {
        match event {
            GamepadEvent::Button { id, pressed } => {
                Self::handle_button(id, pressed, router).await?;
            }
            GamepadEvent::Axis { id, value } => {
                Self::handle_axis(id, value, router, analog_config).await?;
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
    ) -> Result<()> {
        // Extract axis name (e.g., "lx" from "gamepad.axis.lx")
        let axis_name = control_id
            .strip_prefix("gamepad.axis.")
            .ok_or_else(|| anyhow!("Invalid axis control ID: {}", control_id))?;

        // Get analog config
        let analog = match analog_config {
            Some(cfg) => cfg,
            None => {
                // No analog config - use raw value
                match router.handle_control(&control_id, Some(Value::from(raw_value))).await {
                    Ok(_) => debug!("✅ Router handled axis: {} = {:.3}", control_id, raw_value),
                    Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
                }
                return Ok(());
            }
        };

        // Apply deadzone and gamma
        let processed = match process_axis(raw_value, analog) {
            Some(v) => v,
            None => {
                // Within deadzone - send zero
                match router.handle_control(&control_id, Some(Value::from(0.0))).await {
                    Ok(_) => debug!("✅ Router handled axis (deadzone): {} = 0.0", control_id),
                    Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
                }
                return Ok(());
            }
        };

        // Apply inversion
        let final_value = apply_inversion(processed, axis_name, analog);

        debug!("Axis event: {} = {:.3} (raw: {:.3})", control_id, final_value, raw_value);

        // Send to router (pass normalized value, driver will handle scaling)
        match router.handle_control(&control_id, Some(Value::from(final_value))).await {
            Ok(_) => debug!("✅ Router handled axis: {} = {:.3}", control_id, final_value),
            Err(e) => debug!("⚠️  Router error for {}: {}", control_id, e),
        }

        Ok(())
    }

    /// Shutdown the mapper
    pub async fn shutdown(&self) -> Result<()> {
        // Provider will be dropped automatically
        Ok(())
    }
}
