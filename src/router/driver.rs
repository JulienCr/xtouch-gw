//! Driver registration and lifecycle management

use crate::drivers::{Driver, ExecutionContext};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, trace, warn};

impl super::Router {
    /// Create an execution context for driver calls
    pub(crate) async fn create_execution_context(&self) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value: None,
            control_id: None,
            activity_tracker: self.activity_tracker.clone(),
        }
    }

    /// Create an execution context with control information
    pub(crate) async fn create_execution_context_with_control(&self, control_id: String, value: Option<Value>) -> ExecutionContext {
        ExecutionContext {
            config: self.config.clone(),
            active_page: Some(self.get_active_page_name().await),
            value,
            control_id: Some(control_id),
            activity_tracker: self.activity_tracker.clone(),
        }
    }

    /// Register a driver by name (e.g., "voicemeeter", "qlc", "obs")
    ///
    /// The driver will be initialized immediately upon registration
    pub async fn register_driver(&self, name: String, driver: Arc<dyn Driver>) -> Result<()> {
        debug!("Registering driver '{}'...", name);

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

        debug!("Driver '{}' registered and initialized", name);
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
        debug!("Shutting down all drivers...");

        let drivers = self.drivers.read().await;
        let driver_list: Vec<_> = drivers
            .iter()
            .map(|(name, driver)| (name.clone(), driver.clone()))
            .collect();
        drop(drivers);

        let mut errors = Vec::new();
        for (name, driver) in driver_list {
            debug!("Shutting down driver '{}'...", name);
            if let Err(e) = driver.shutdown().await {
                warn!("Failed to shutdown driver '{}': {}", name, e);
                errors.push((name, e));
            } else {
                debug!("Driver '{}' shut down", name);
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
        debug!("All drivers shut down successfully");
        Ok(())
    }

    /// Handle a control event (resolve mapping and execute driver action)
    ///
    /// # Arguments
    /// * `control_id` - The control identifier (e.g., "gamepad1.btn.a")
    /// * `value` - Optional value for the control (e.g., axis position)
    /// * `extra_params` - Optional extra parameters to append to the mapping params
    pub async fn handle_control(
        &self,
        control_id: &str,
        value: Option<Value>,
        extra_params: Option<Vec<Value>>,
    ) -> Result<()> {
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
        let raw_params = mapping.params.clone().unwrap_or_default();

        // Drop config lock before async operations
        drop(config);

        // Resolve $camera placeholders for dynamic gamepad targeting
        let mut params = self.resolve_camera_params(control_id, raw_params).await?;

        // Append extra parameters if provided (e.g., target="preview")
        if let Some(extra) = extra_params {
            params.extend(extra);
        }

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

    /// Resolve $camera.* placeholders in action params
    ///
    /// For gamepads in "dynamic" mode, replaces placeholders with actual camera config.
    /// For "static" mode or non-gamepad controls, returns params unchanged.
    async fn resolve_camera_params(
        &self,
        control_id: &str,
        raw_params: Vec<Value>,
    ) -> Result<Vec<Value>> {
        // Check if any param contains $camera placeholder
        let has_camera_placeholder = raw_params
            .iter()
            .any(|p| p.as_str().map(|s| s.contains("$camera")).unwrap_or(false));

        if !has_camera_placeholder {
            return Ok(raw_params);
        }

        // Extract gamepad slot from control_id (e.g., "gamepad1" from "gamepad1.axis.lx")
        let gamepad_slot = control_id.split('.').next().unwrap_or("");

        if !gamepad_slot.starts_with("gamepad") {
            // Not a gamepad control, return unchanged
            return Ok(raw_params);
        }

        let config = self.config.read().await;

        // Find the gamepad slot config
        let slot_config = config.gamepad.as_ref().and_then(|g| g.gamepads.as_ref()).and_then(
            |slots| {
                // Determine slot index from gamepad_slot (gamepad1 -> index 0, gamepad2 -> index 1)
                let slot_num: usize = gamepad_slot
                    .strip_prefix("gamepad")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                slots.get(slot_num.saturating_sub(1))
            },
        );

        // Get camera target mode
        let camera_target_mode = slot_config
            .and_then(|c| c.camera_target.as_ref())
            .map(|s| s.as_str())
            .unwrap_or("static");

        // Determine which camera to use
        let camera_id = match camera_target_mode {
            "static" => {
                // Static mode: no substitution
                return Ok(raw_params);
            }
            "dynamic" => {
                // Dynamic mode: get from runtime state, fallback to first camera
                self.camera_targets.get_target(gamepad_slot).or_else(|| {
                    // Fallback: use first camera from config
                    config
                        .obs
                        .as_ref()
                        .and_then(|o| o.camera_control.as_ref())
                        .and_then(|cc| cc.cameras.first())
                        .map(|c| c.id.clone())
                })
                .ok_or_else(|| {
                    anyhow!(
                        "No camera target for {} and no cameras configured",
                        gamepad_slot
                    )
                })?
            }
            fixed_camera_id => {
                // Fixed to specific camera
                fixed_camera_id.to_string()
            }
        };

        // Find camera config
        let camera_config = config
            .obs
            .as_ref()
            .and_then(|o| o.camera_control.as_ref())
            .and_then(|cc| cc.cameras.iter().find(|c| c.id == camera_id))
            .ok_or_else(|| anyhow!("Camera '{}' not found in camera_control config", camera_id))?;

        // Clone camera values before dropping config
        let scene = camera_config.scene.clone();
        let source = camera_config.source.clone();
        let split_source = camera_config.split_source.clone();
        let id = camera_config.id.clone();

        drop(config);

        // Resolve placeholders
        let resolved: Vec<Value> = raw_params
            .into_iter()
            .map(|param| {
                if let Some(s) = param.as_str() {
                    // First replace specific $camera.* placeholders
                    let replaced = s
                        .replace("$camera.scene", &scene)
                        .replace("$camera.source", &source)
                        .replace("$camera.split_source", &split_source)
                        .replace("$camera.id", &id);

                    // Then replace standalone $camera with camera_id
                    // (must be after $camera.* to avoid partial replacement)
                    let replaced = replaced.replace("$camera", &id);

                    if replaced != s {
                        trace!("Resolved camera param: '{}' -> '{}'", s, replaced);
                    }

                    Value::String(replaced)
                } else {
                    param
                }
            })
            .collect();

        debug!(
            "Resolved $camera params for {} (camera={}): {:?}",
            control_id, camera_id, resolved
        );

        Ok(resolved)
    }
}

