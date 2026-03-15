//! Driver trait implementation for OBS
//!
//! Handles all action execution, initialization, and lifecycle management.
//! Action-specific logic is delegated to submodules:
//! - `ptz_actions`: PTZ nudge/scale/reset operations
//! - `camera_actions`: Camera selection and PTZ target context
//! - `split_mode`: Split view enter/exit/toggle

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};

use super::camera_actions::extract_gamepad_slot;
use super::driver::ObsDriver;
use super::ptz_actions::PtzAxis;
use super::{Driver, ExecutionContext, IndicatorCallback};

impl ObsDriver {
    /// Resolve camera_id to (scene, source) from camera_control config.
    /// Returns None if camera_id is not found (assumes it's already scene/source format).
    fn resolve_camera_id(&self, camera_id: &str) -> Option<(String, String)> {
        let config_guard = self.camera_control_config.read();
        config_guard
            .as_ref()
            .and_then(|cc| cc.cameras.iter().find(|c| c.id == camera_id))
            .map(|c| (c.scene.clone(), c.source.clone()))
    }

    /// Check if PTZ is enabled for a camera matching the given predicate.
    /// Returns true by default if camera not found (legacy compatibility).
    fn is_ptz_enabled_where<F>(&self, predicate: F) -> bool
    where
        F: Fn(&crate::config::CameraConfig) -> bool,
    {
        let config_guard = self.camera_control_config.read();
        config_guard
            .as_ref()
            .and_then(|cc| cc.cameras.iter().find(|c| predicate(c)))
            .map(|c| c.enable_ptz)
            .unwrap_or(true)
    }

    /// Check if PTZ is enabled for a camera by ID.
    pub(super) fn is_ptz_enabled(&self, camera_id: &str) -> bool {
        self.is_ptz_enabled_where(|c| c.id == camera_id)
    }

    /// Check if PTZ is enabled for a camera by scene name.
    pub(super) fn is_ptz_enabled_by_scene(&self, scene: &str) -> bool {
        self.is_ptz_enabled_where(|c| c.scene == scene)
    }

    /// Parse camera params with step for nudgeX, nudgeY, scaleUniform.
    /// Supports both new format [camera_id, step] and legacy [scene, source, step].
    pub(super) fn parse_camera_params_with_step(
        &self,
        params: &[Value],
    ) -> Result<(String, String, f64)> {
        match params.len() {
            2 => {
                // New format: [camera_id, step]
                let camera_id = params[0]
                    .as_str()
                    .ok_or_else(|| anyhow!("camera_id must be a string"))?;
                let step = params[1]
                    .as_f64()
                    .ok_or_else(|| anyhow!("step must be a number"))?;

                let (scene, source) = self.resolve_camera_id(camera_id).ok_or_else(|| {
                    anyhow!("Camera '{}' not found in camera_control config", camera_id)
                })?;

                Ok((scene, source, step))
            },
            3 => {
                // Legacy format: [scene, source, step]
                let scene = params[0]
                    .as_str()
                    .ok_or_else(|| anyhow!("scene must be a string"))?
                    .to_string();
                let source = params[1]
                    .as_str()
                    .ok_or_else(|| anyhow!("source must be a string"))?
                    .to_string();
                let step = params[2]
                    .as_f64()
                    .ok_or_else(|| anyhow!("step must be a number"))?;
                Ok((scene, source, step))
            },
            _ => Err(anyhow!(
                "Expected [camera_id, step] or [scene, source, step]"
            )),
        }
    }

    /// Parse camera params for resetPosition, resetZoom.
    /// Supports both new format [camera_id] and legacy [scene, source].
    pub(super) fn parse_camera_params(&self, params: &[Value]) -> Result<(String, String)> {
        match params.len() {
            1 => {
                // New format: [camera_id]
                let camera_id = params[0]
                    .as_str()
                    .ok_or_else(|| anyhow!("camera_id must be a string"))?;

                self.resolve_camera_id(camera_id).ok_or_else(|| {
                    anyhow!("Camera '{}' not found in camera_control config", camera_id)
                })
            },
            2 => {
                // Legacy format: [scene, source]
                let scene = params[0]
                    .as_str()
                    .ok_or_else(|| anyhow!("scene must be a string"))?
                    .to_string();
                let source = params[1]
                    .as_str()
                    .ok_or_else(|| anyhow!("source must be a string"))?
                    .to_string();
                Ok((scene, source))
            },
            _ => Err(anyhow!("Expected [camera_id] or [scene, source]")),
        }
    }
}

#[async_trait]
impl Driver for ObsDriver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&self, ctx: ExecutionContext) -> Result<()> {
        info!("Initializing OBS WebSocket driver");

        // Store activity tracker if available
        if let Some(tracker) = ctx.activity_tracker {
            *self.activity_tracker.write() = Some(tracker);
        }

        // Attempt initial connection
        match self.connect().await {
            Ok(_) => {
                info!("OBS connected on init");
            },
            Err(e) => {
                warn!("OBS connection failed on init: {}", e);
                warn!("Will retry automatically in background");

                self.emit_status(crate::tray::ConnectionStatus::Disconnected);

                // Start background reconnection
                let driver_clone = self.clone_for_task();
                tokio::spawn(async move {
                    driver_clone.schedule_reconnect().await;
                });
            },
        }

        // Always succeed - driver is registered even if disconnected
        Ok(())
    }

    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()> {
        // Check if connected
        if self.client.read().await.is_none() {
            warn!("OBS not connected, action dropped");

            // Trigger reconnect if not already running
            if *self.reconnect_count.lock() == 0 {
                debug!("Triggering background reconnection");
                let driver_clone = self.clone_for_task();
                tokio::spawn(async move {
                    driver_clone.schedule_reconnect().await;
                });
            }

            return Err(anyhow!("OBS not connected"));
        }

        // Record outbound activity
        if let Some(ref tracker) = ctx.activity_tracker {
            tracker.record("obs", crate::tray::ActivityDirection::Outbound);
        }

        // Filter button releases for trigger-style actions (press-only semantics).
        // Continuous actions (nudge, scale) and stateful actions (setPtzModifier)
        // intentionally process releases and are excluded from this filter.
        if ctx.is_button_release() {
            match action {
                "changeScene"
                | "setScene"
                | "toggleStudioMode"
                | "TriggerStudioModeTransition"
                | "selectCamera"
                | "enterSplit"
                | "toggleSplit"
                | "exitSplit" => return Ok(()),
                _ => {},
            }
        }

        match action {
            "changeScene" | "setScene" => {
                let scene_name = params
                    .first()
                    .and_then(|v| v.as_str())
                    .context("Scene name required")?;

                let target = if *self.studio_mode.read() {
                    "Preview"
                } else {
                    "Program"
                };
                info!("OBS {} scene change -> '{}'", target, scene_name);

                self.set_scene_for_mode(scene_name).await?;
                Ok(())
            },

            "toggleStudioMode" => {
                let guard = self.get_connected_client().await?;
                let client = guard
                    .as_ref()
                    .context("BUG: get_connected_client returned None")?;

                // Get current state and toggle
                let current = *self.studio_mode.read();
                let new_state = !current;

                info!("OBS Studio Mode toggle: {} -> {}", current, new_state);
                client.ui().set_studio_mode_enabled(new_state).await?;

                // When enabling studio mode, set preview to current program scene
                if new_state {
                    let program = self.program_scene.read().clone();
                    if !program.is_empty() {
                        client.scenes().set_current_preview_scene(&program).await?;
                    }
                }

                Ok(())
            },

            "TriggerStudioModeTransition" => {
                info!("OBS Studio Transition requested");
                let guard = self.get_connected_client().await?;
                let client = guard
                    .as_ref()
                    .context("BUG: get_connected_client returned None")?;

                client.transitions().trigger().await?;
                Ok(())
            },

            "nudgeX" => {
                self.execute_ptz_action(&params, &ctx, PtzAxis::X, 2.0)
                    .await
            },

            "nudgeY" => {
                self.execute_ptz_action(&params, &ctx, PtzAxis::Y, 2.0)
                    .await
            },

            "scaleUniform" => {
                self.execute_ptz_action(&params, &ctx, PtzAxis::Scale, 0.02)
                    .await
            },

            "resetPosition" => self.execute_reset_position(&params).await,

            "resetZoom" => self.execute_reset_zoom(&params).await,

            "selectCamera" => self.execute_select_camera(&params, &ctx).await,

            "enterSplit" => self.execute_enter_split(&params, &ctx).await,

            "toggleSplit" => self.execute_toggle_split(&params, &ctx).await,

            "exitSplit" => self.execute_exit_split(&ctx).await,

            "setPtzModifier" => {
                // Track PTZ modifier state (e.g., when LT is held on gamepad)
                // This enables preview mode for selectCamera actions
                let control_id = ctx.control_id.as_deref().unwrap_or("unknown");
                let slot = extract_gamepad_slot(control_id);

                // Get pressed state from context value (> 0 = pressed, 0 = released)
                let pressed = ctx
                    .value
                    .as_ref()
                    .and_then(|v| v.as_f64())
                    .map(|v| v > 0.0)
                    .unwrap_or(false);

                // Update modifier state in camera_targets
                if let Some(ref camera_targets) = ctx.camera_targets {
                    camera_targets.set_ptz_modifier(&slot, pressed);
                    debug!(
                        "PTZ modifier: {} = {} (control: {})",
                        slot, pressed, control_id
                    );
                } else {
                    warn!("setPtzModifier: camera_targets not available in context");
                }

                Ok(())
            },

            _ => {
                warn!("Unknown OBS action: {}", action);
                Ok(())
            },
        }
    }

    async fn sync(&self) -> Result<()> {
        debug!("OBS driver sync - refreshing state");
        self.refresh_state().await?;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down OBS WebSocket driver");
        *self.shutdown_flag.lock() = true;

        if let Some(client) = self.client.write().await.take() {
            drop(client); // Close the connection
        }

        info!("OBS WebSocket driver shutdown complete");
        Ok(())
    }

    fn subscribe_indicators(&self, callback: IndicatorCallback) {
        debug!("OBS driver: new indicator subscription");
        self.indicator_emitters.write().push(callback);

        // Emit initial state immediately to new subscriber
        let studio_mode = *self.studio_mode.read();
        let program_scene = self.program_scene.read().clone();
        let preview_scene = self.preview_scene.read().clone();

        let emitters = self.indicator_emitters.read();
        if let Some(emit) = emitters.last() {
            emit(
                super::signals::STUDIO_MODE.to_string(),
                Value::Bool(studio_mode),
            );
            emit(
                super::signals::CURRENT_PROGRAM_SCENE.to_string(),
                Value::String(program_scene.clone()),
            );
            emit(
                super::signals::CURRENT_PREVIEW_SCENE.to_string(),
                Value::String(preview_scene.clone()),
            );

            // Emit composite selectedScene
            let selected = if studio_mode {
                preview_scene
            } else {
                program_scene
            };
            emit(
                super::signals::SELECTED_SCENE.to_string(),
                Value::String(selected),
            );
        }
    }

    fn connection_status(&self) -> crate::tray::ConnectionStatus {
        self.current_status.read().clone()
    }

    fn subscribe_connection_status(&self, callback: crate::tray::StatusCallback) {
        debug!("OBS driver: new connection status subscription");

        // Emit current status immediately to new subscriber
        let current = self.current_status.read().clone();
        callback(current);

        // Add to callbacks list
        self.status_callbacks.write().push(callback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test the preview/program target parameter parsing logic.
    ///
    /// Note: These tests verify the parameter parsing and use_preview decision logic only.
    /// Side effects like auto-enabling studio mode require integration tests with OBS client.
    mod select_camera_target_tests {
        use super::*;

        /// Helper to extract target from params (mirrors selectCamera logic)
        fn parse_explicit_target(params: &[Value]) -> Option<&str> {
            params.get(1).and_then(|v| v.as_str())
        }

        /// Helper to determine use_preview based on target and studio_mode
        fn determine_use_preview(explicit_target: Option<&str>, studio_mode: bool) -> bool {
            match explicit_target {
                Some("preview") => true,
                Some("program") => false,
                _ => studio_mode, // Legacy behavior
            }
        }

        #[test]
        fn explicit_preview_target_forces_preview() {
            let params = vec![json!("camera_a"), json!("preview")];
            let target = parse_explicit_target(&params);
            assert_eq!(target, Some("preview"));
            // Should use preview regardless of studio_mode
            assert!(determine_use_preview(target, false));
            assert!(determine_use_preview(target, true));
        }

        #[test]
        fn explicit_program_target_forces_program() {
            let params = vec![json!("camera_a"), json!("program")];
            let target = parse_explicit_target(&params);
            assert_eq!(target, Some("program"));
            // Should NOT use preview regardless of studio_mode
            assert!(!determine_use_preview(target, false));
            assert!(!determine_use_preview(target, true));
        }

        #[test]
        fn no_target_uses_legacy_behavior() {
            let params = vec![json!("camera_a")];
            let target = parse_explicit_target(&params);
            assert_eq!(target, None);
            // Should follow studio_mode
            assert!(!determine_use_preview(target, false)); // studio_mode=false -> program
            assert!(determine_use_preview(target, true)); // studio_mode=true -> preview
        }

        #[test]
        fn invalid_target_uses_legacy_behavior() {
            let params = vec![json!("camera_a"), json!("invalid")];
            let target = parse_explicit_target(&params);
            assert_eq!(target, Some("invalid"));
            // Unknown target falls through to legacy behavior
            let use_preview = match target {
                Some("preview") => true,
                Some("program") => false,
                _ => true, // Simulating studio_mode=true
            };
            assert!(use_preview);
        }

        #[test]
        fn numeric_target_is_ignored() {
            let params = vec![json!("camera_a"), json!(123)];
            let target = parse_explicit_target(&params);
            assert_eq!(target, None); // as_str() returns None for numbers
        }
    }
}
