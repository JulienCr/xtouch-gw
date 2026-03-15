//! Split mode actions for OBS
//!
//! Handles `enterSplit`, `toggleSplit`, and `exitSplit` action execution.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tracing::info;

use super::camera::ViewMode;
use super::camera_actions::PtzTargetContext;
use super::driver::ObsDriver;
use super::ExecutionContext;

impl ObsDriver {
    /// Execute the `enterSplit` action.
    ///
    /// Switches to a split view scene and sets the last-used camera in the split.
    pub(super) async fn execute_enter_split(
        &self,
        params: &[Value],
        ctx: &ExecutionContext,
    ) -> Result<()> {
        let side = params
            .first()
            .and_then(|v| v.as_str())
            .context("Side required ('left' or 'right')")?;

        let (split_scene, new_mode, last_camera) = {
            let config_guard = self.camera_control_config.read();
            let config = config_guard
                .as_ref()
                .context("Camera control not configured")?;

            // Determine split scene
            let (split_scene, new_mode) = match side {
                "left" => (config.splits.left.clone(), ViewMode::SplitLeft),
                "right" => (config.splits.right.clone(), ViewMode::SplitRight),
                _ => {
                    return Err(anyhow!(
                        "Invalid side '{}', must be 'left' or 'right'",
                        side
                    ))
                },
            };

            let last_camera = self.camera_control_state.read().last_camera.clone();

            // If last_camera is empty, use first camera
            let last_camera = if last_camera.is_empty() {
                config
                    .cameras
                    .first()
                    .map(|c| c.id.clone())
                    .unwrap_or_else(|| "Main".to_string())
            } else {
                last_camera
            };

            (split_scene, new_mode, last_camera)
        };

        info!("OBS: Enter split '{}' -> scene '{}'", side, split_scene);

        // Switch to split scene
        self.set_scene_for_mode(&split_scene).await?;

        // Update state BEFORE setting camera (so state is updated even if camera fails)
        {
            let mut state = self.camera_control_state.write();
            state.current_view_mode = new_mode;
            state.last_camera = last_camera.clone();
        }

        // Set the camera in the split
        self.set_split_camera(&split_scene, &last_camera).await?;

        // Set PTZ target for the displayed camera
        let ptz_enabled = self.is_ptz_enabled(&last_camera);
        PtzTargetContext::from_ctx(ctx).set_ptz_target(&last_camera, ptz_enabled, "enterSplit");

        Ok(())
    }

    /// Execute the `toggleSplit` action.
    ///
    /// Toggles between full and split mode. If already in any split mode,
    /// exits to full; otherwise enters the requested split side.
    pub(super) async fn execute_toggle_split(
        &self,
        params: &[Value],
        ctx: &ExecutionContext,
    ) -> Result<()> {
        let side = params
            .first()
            .and_then(|v| v.as_str())
            .context("Side required ('left' or 'right')")?;

        // Validate side parameter
        let target_mode = match side {
            "left" => ViewMode::SplitLeft,
            "right" => ViewMode::SplitRight,
            _ => {
                return Err(anyhow!(
                    "Invalid side '{}', must be 'left' or 'right'",
                    side
                ))
            },
        };

        // Read current mode and optimistically update to prevent double-toggle race
        let should_exit = {
            let mut state = self.camera_control_state.write();
            if state.current_view_mode != ViewMode::Full {
                // Already in split -> mark as going to Full
                state.current_view_mode = ViewMode::Full;
                true
            } else {
                // Full -> mark as going to target split
                state.current_view_mode = target_mode;
                false
            }
        };

        if should_exit {
            self.execute_exit_split(ctx).await
        } else {
            self.execute_enter_split(params, ctx).await
        }
    }

    /// Execute the `exitSplit` action.
    ///
    /// Returns from split view to full mode, switching to the default camera scene.
    pub(super) async fn execute_exit_split(&self, ctx: &ExecutionContext) -> Result<()> {
        let (target_camera, camera_scene) = {
            let config_guard = self.camera_control_config.read();
            let config = config_guard
                .as_ref()
                .context("Camera control not configured")?;

            // Use default_camera if configured, otherwise first camera
            let target_camera = config
                .default_camera
                .clone()
                .or_else(|| config.cameras.first().map(|c| c.id.clone()))
                .unwrap_or_else(|| "Main".to_string());

            let camera = config
                .cameras
                .iter()
                .find(|c| c.id == target_camera)
                .or_else(|| config.cameras.first())
                .context("No cameras configured")?;

            (target_camera, camera.scene.clone())
        };

        info!(
            "OBS: Exit split -> camera '{}' scene '{}'",
            target_camera, camera_scene
        );

        // Switch to full scene
        self.set_scene_for_mode(&camera_scene).await?;

        // Update state
        {
            let mut state = self.camera_control_state.write();
            state.current_view_mode = ViewMode::Full;
            state.last_camera = target_camera.clone();
        }

        // Set PTZ target to match the displayed camera
        let ptz_enabled = self.is_ptz_enabled(&target_camera);
        PtzTargetContext::from_ctx(ctx).set_ptz_target(&target_camera, ptz_enabled, "exitSplit");

        Ok(())
    }
}
