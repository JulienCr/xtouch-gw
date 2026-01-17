//! Driver trait implementation for OBS
//!
//! Handles all action execution, initialization, and lifecycle management.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, info, warn};

use super::analog::shape_analog;
use super::camera::ViewMode;
use super::driver::ObsDriver;
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
    fn is_ptz_enabled(&self, camera_id: &str) -> bool {
        self.is_ptz_enabled_where(|c| c.id == camera_id)
    }

    /// Check if PTZ is enabled for a camera by scene name.
    fn is_ptz_enabled_by_scene(&self, scene: &str) -> bool {
        self.is_ptz_enabled_where(|c| c.scene == scene)
    }

    /// Parse camera params with step for nudgeX, nudgeY, scaleUniform.
    /// Supports both new format [camera_id, step] and legacy [scene, source, step].
    fn parse_camera_params_with_step(&self, params: &[Value]) -> Result<(String, String, f64)> {
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
    fn parse_camera_params(&self, params: &[Value]) -> Result<(String, String)> {
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

    /// Convert a MIDI encoder value into a signed directional delta for PTZ operations.
    ///
    /// Standard encoder values: 0/64=no change, 1-63=clockwise, 65-127=counter-clockwise.
    ///
    /// # Arguments
    /// * `value` - The MIDI encoder value as a JSON Value
    /// * `step` - The amount to move per encoder tick
    ///
    /// # Returns
    /// Returns `step` for clockwise input, `-step` for counter-clockwise, or `0.0` for no movement.
    fn encoder_value_to_delta(value: &Value, step: f64) -> f64 {
        let v = match value.as_f64() {
            Some(v) => v,
            None => return 0.0,
        };

        if (1.0..=63.0).contains(&v) {
            step
        } else if (65.0..=127.0).contains(&v) {
            -step
        } else {
            0.0
        }
    }

    /// Handle gamepad analog input for PTZ operations.
    ///
    /// Processes raw gamepad value with gamma shaping and gain scaling,
    /// then sets the appropriate analog rate for continuous movement.
    /// This function updates the internal analog rate state directly and does not return a value.
    fn handle_gamepad_analog(
        &self,
        value: Option<&Value>,
        scene: &str,
        source: &str,
        step: f64,
        gain: f64,
        axis: PtzAxis,
    ) {
        let raw_value = match value {
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        };

        if let Some(v) = raw_value {
            let clamped = v.clamp(-1.0, 1.0);
            let gamma = *self.analog_gamma.read();
            let shaped = shape_analog(clamped, gamma);
            let velocity = shaped * step * gain;

            match axis {
                PtzAxis::X => self.set_analog_rate(scene, source, Some(velocity), None, None),
                PtzAxis::Y => self.set_analog_rate(scene, source, None, Some(velocity), None),
                PtzAxis::Scale => self.set_analog_rate(scene, source, None, None, Some(velocity)),
            }
        }
    }

    /// Handle encoder input for PTZ operations with acceleration.
    ///
    /// Processes encoder delta with acceleration tracking and applies
    /// the resulting delta to the scene transform.
    async fn handle_encoder_ptz(
        &self,
        ctx: &ExecutionContext,
        scene: &str,
        source: &str,
        step: f64,
        axis: PtzAxis,
    ) -> Result<()> {
        let delta = match &ctx.value {
            Some(value) => Self::encoder_value_to_delta(value, step),
            None => step,
        };

        if delta == 0.0 {
            return Ok(());
        }

        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
        let final_delta = delta * accel;

        debug!(
            "OBS {:?} encoder: id='{}' delta={} accel={:.2}x final={:.2}",
            axis, control_id, delta, accel, final_delta
        );

        match axis {
            PtzAxis::X => {
                self.apply_delta(scene, source, Some(final_delta), None, None)
                    .await
            },
            PtzAxis::Y => {
                self.apply_delta(scene, source, None, Some(final_delta), None)
                    .await
            },
            PtzAxis::Scale => {
                self.apply_delta(scene, source, None, None, Some(final_delta))
                    .await
            },
        }
    }

    /// Execute a PTZ nudge/scale action (common logic for nudgeX, nudgeY, scaleUniform).
    async fn execute_ptz_action(
        &self,
        params: &[Value],
        ctx: &ExecutionContext,
        axis: PtzAxis,
        default_step: f64,
    ) -> Result<()> {
        let (scene_name, source_name, step) = self
            .parse_camera_params_with_step(params)
            .unwrap_or_else(|_| {
                let scene = params
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let source = params
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (scene, source, default_step)
            });

        if !self.is_ptz_enabled_by_scene(&scene_name) {
            return Ok(());
        }

        let is_gamepad = ctx
            .control_id
            .as_ref()
            .map(|id| id.starts_with("gamepad"))
            .unwrap_or(false);

        if is_gamepad {
            let gain = match axis {
                PtzAxis::X | PtzAxis::Y => *self.analog_pan_gain.read(),
                PtzAxis::Scale => *self.analog_zoom_gain.read(),
            };
            self.handle_gamepad_analog(
                ctx.value.as_ref(),
                &scene_name,
                &source_name,
                step,
                gain,
                axis,
            );
        } else {
            self.handle_encoder_ptz(ctx, &scene_name, &source_name, step, axis)
                .await?;
        }

        Ok(())
    }
}

/// PTZ axis for nudge/scale operations
#[derive(Debug, Clone, Copy)]
enum PtzAxis {
    X,
    Y,
    Scale,
}

#[async_trait]
impl Driver for ObsDriver {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init(&self, ctx: ExecutionContext) -> Result<()> {
        info!("🎬 Initializing OBS WebSocket driver");

        // Store activity tracker if available
        if let Some(tracker) = ctx.activity_tracker {
            *self.activity_tracker.write() = Some(tracker);
        }

        // Attempt initial connection
        match self.connect().await {
            Ok(_) => {
                info!("✅ OBS connected on init");
            },
            Err(e) => {
                warn!("⚠️  OBS connection failed on init: {}", e);
                warn!("🔄 Will retry automatically in background");

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
            warn!("⚠️  OBS not connected, action dropped");

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
                info!("🎬 OBS {} scene change -> '{}'", target, scene_name);

                self.set_scene_for_mode(scene_name).await?;
                Ok(())
            },

            "toggleStudioMode" => {
                let guard = self.client.read().await;
                let client = guard.as_ref().context("OBS not connected")?;

                // Get current state and toggle
                let current = *self.studio_mode.read();
                let new_state = !current;

                info!("🎬 OBS Studio Mode toggle: {} → {}", current, new_state);
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
                info!("🎬 OBS Studio Transition requested");
                let guard = self.client.read().await;
                let client = guard.as_ref().context("OBS not connected")?;

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

            "resetPosition" => {
                // Parse params: [camera_id] or [scene, source]
                let (scene_name, source_name) = self
                    .parse_camera_params(&params)
                    .context("resetPosition requires [camera_id] or [scene, source]")?;

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                info!(
                    "OBS Reset position: scene='{}' source='{}'",
                    scene_name, source_name
                );

                // Stop any analog motion on position axes
                self.set_analog_rate(&scene_name, &source_name, Some(0.0), Some(0.0), None);

                // Resolve item ID
                let item_id = self.resolve_item_id(&scene_name, &source_name).await?;

                // Get canvas dimensions
                let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;

                // Build transform to reset position to center
                let mut transform = obws::requests::scene_items::SceneItemTransform::default();

                // Set alignment to center (so position refers to object center, not corner)
                transform.alignment = Some(obws::common::Alignment::CENTER);

                // Set position to canvas center
                transform.position = Some(obws::requests::scene_items::Position {
                    x: Some((canvas_width / 2.0) as f32),
                    y: Some((canvas_height / 2.0) as f32),
                    ..Default::default()
                });

                // Send to OBS
                let guard = self.client.read().await;
                let client = guard.as_ref().context("OBS client not connected")?;

                client
                    .scene_items()
                    .set_transform(obws::requests::scene_items::SetTransform {
                        scene: &scene_name,
                        item_id,
                        transform,
                    })
                    .await
                    .context("Failed to reset scene item position")?;

                // Update cache with new position and alignment
                let cache_key = self.cache_key(&scene_name, &source_name);
                if let Some(state) = self.transform_cache.write().get_mut(&cache_key) {
                    state.x = canvas_width / 2.0;
                    state.y = canvas_height / 2.0;
                    state.alignment = 0; // CENTER
                }

                debug!(
                    "OBS reset position: '{}' position=({:.1},{:.1}) alignment=CENTER",
                    self.cache_key(&scene_name, &source_name),
                    canvas_width / 2.0,
                    canvas_height / 2.0
                );

                Ok(())
            },

            "resetZoom" => {
                // Parse params: [camera_id] or [scene, source]
                let (scene_name, source_name) = self
                    .parse_camera_params(&params)
                    .context("resetZoom requires [camera_id] or [scene, source]")?;

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                info!(
                    "OBS Reset zoom: scene='{}' source='{}'",
                    scene_name, source_name
                );

                // Stop any analog motion on zoom axis
                self.set_analog_rate(&scene_name, &source_name, None, None, Some(0.0));

                // Resolve item ID
                let item_id = self.resolve_item_id(&scene_name, &source_name).await?;

                // Read current transform from OBS (not cache!) to determine type
                let current = self.read_transform(&scene_name, item_id).await?;

                // Detect if this source uses bounds-based or scale-based transform
                let is_bounds_based = matches!(
                    (current.bounds_width, current.bounds_height),
                    (Some(bw), Some(bh)) if bw > 0.0 && bh > 0.0
                );

                // Build transform to reset zoom
                let mut transform = obws::requests::scene_items::SceneItemTransform::default();

                if is_bounds_based {
                    // For bounds-based sources (NDI cameras): reset bounds to canvas dimensions
                    // Keep position unchanged - user can reset position separately with resetPosition
                    let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;

                    transform.bounds = Some(obws::requests::scene_items::Bounds {
                        width: Some(canvas_width as f32),
                        height: Some(canvas_height as f32),
                        ..Default::default()
                    });

                    info!(
                        "OBS Reset zoom (bounds): {}x{}",
                        canvas_width as u32, canvas_height as u32
                    );
                } else {
                    // For scale-based sources: reset scale to 1.0
                    transform.scale = Some(obws::requests::scene_items::Scale {
                        x: Some(1.0),
                        y: Some(1.0),
                        ..Default::default()
                    });
                    debug!("OBS Reset zoom (scale): 1.0x1.0");
                }

                // Send to OBS
                let guard = self.client.read().await;
                let client = guard.as_ref().context("OBS client not connected")?;

                client
                    .scene_items()
                    .set_transform(obws::requests::scene_items::SetTransform {
                        scene: &scene_name,
                        item_id,
                        transform,
                    })
                    .await
                    .context("Failed to reset scene item zoom")?;

                // Invalidate cache to force fresh read on next operation
                let cache_key = self.cache_key(&scene_name, &source_name);
                self.transform_cache.write().remove(&cache_key);

                debug!("OBS reset zoom: '{}' (cache invalidated)", cache_key);

                Ok(())
            },

            "selectCamera" => {
                let camera_id = params
                    .first()
                    .and_then(|v| v.as_str())
                    .context("Camera ID required")?;

                // Parse optional target parameter: "preview", "program", or absent (legacy behavior)
                let explicit_target = params.get(1).and_then(|v| v.as_str());

                // Get view mode and resolve scene names from config
                let view_mode = self.camera_control_state.read().current_view_mode;
                let (camera_scene, split_scene) = {
                    let config_guard = self.camera_control_config.read();
                    let config = config_guard
                        .as_ref()
                        .context("Camera control not configured")?;

                    let camera = config
                        .cameras
                        .iter()
                        .find(|c| c.id == camera_id)
                        .with_context(|| format!("Camera '{}' not found", camera_id))?;

                    let split = match view_mode {
                        ViewMode::Full => None,
                        ViewMode::SplitLeft => Some(config.splits.left.clone()),
                        ViewMode::SplitRight => Some(config.splits.right.clone()),
                    };

                    (camera.scene.clone(), split)
                };

                match view_mode {
                    ViewMode::Full => {
                        let guard = self.client.read().await;
                        let client = guard.as_ref().context("OBS not connected")?;

                        // Determine whether to use preview or program
                        let use_preview = match explicit_target {
                            Some("preview") => {
                                // Force preview mode - auto-enable studio mode if needed
                                if !*self.studio_mode.read() {
                                    info!("Enabling studio mode for preview operation");
                                    client.ui().set_studio_mode_enabled(true).await?;
                                    *self.studio_mode.write() = true;
                                }
                                true
                            },
                            Some("program") => false,
                            _ => *self.studio_mode.read(), // Legacy behavior
                        };

                        let target_name = if use_preview { "preview" } else { "program" };
                        info!(
                            "🎬 OBS: Select camera '{}' (FULL mode) → {} '{}'",
                            camera_id, target_name, camera_scene
                        );

                        if use_preview {
                            client
                                .scenes()
                                .set_current_preview_scene(&camera_scene)
                                .await?;
                        } else {
                            client
                                .scenes()
                                .set_current_program_scene(&camera_scene)
                                .await?;
                        }
                    },
                    ViewMode::SplitLeft | ViewMode::SplitRight => {
                        let split_scene =
                            split_scene.expect("split_scene must be Some for split modes");

                        // Note: explicit_target is ignored in split mode (modifies sources, not scenes)
                        if let Some(target) = explicit_target {
                            debug!("🎬 OBS: Ignoring target '{}' in SPLIT mode (split modifies sources, not scenes)", target);
                        }

                        info!(
                            "🎬 OBS: Select camera '{}' (SPLIT mode) in '{}'",
                            camera_id, split_scene
                        );
                        self.set_split_camera(&split_scene, camera_id).await?;
                    },
                }

                // Update last_camera for all modes
                self.camera_control_state.write().last_camera = camera_id.to_string();

                Ok(())
            },

            "enterSplit" => {
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

                info!("🎬 OBS: Enter split '{}' -> scene '{}'", side, split_scene);

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

                Ok(())
            },

            "exitSplit" => {
                let (last_camera, camera_scene) = {
                    let config_guard = self.camera_control_config.read();
                    let config = config_guard
                        .as_ref()
                        .context("Camera control not configured")?;

                    // Find last camera scene
                    let last_camera = self.camera_control_state.read().last_camera.clone();
                    let camera = config
                        .cameras
                        .iter()
                        .find(|c| c.id == last_camera)
                        .or_else(|| config.cameras.first())
                        .context("No cameras configured")?;

                    (last_camera, camera.scene.clone())
                };

                info!(
                    "🎬 OBS: Exit split -> camera '{}' scene '{}'",
                    last_camera, camera_scene
                );

                // Switch to full scene
                self.set_scene_for_mode(&camera_scene).await?;

                // Update state
                self.camera_control_state.write().current_view_mode = ViewMode::Full;

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

        info!("✅ OBS WebSocket driver shutdown complete");
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
            emit("obs.studioMode".to_string(), Value::Bool(studio_mode));
            emit(
                "obs.currentProgramScene".to_string(),
                Value::String(program_scene.clone()),
            );
            emit(
                "obs.currentPreviewScene".to_string(),
                Value::String(preview_scene.clone()),
            );

            // Emit composite selectedScene
            let selected = if studio_mode {
                preview_scene
            } else {
                program_scene
            };
            emit("obs.selectedScene".to_string(), Value::String(selected));
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

    /// Test encoder_value_to_delta with various MIDI encoder values.
    /// Standard encoder protocol: 0/64=no change, 1-63=clockwise, 65-127=counter-clockwise
    mod encoder_value_to_delta_tests {
        use super::*;

        #[test]
        fn zero_returns_no_change() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(0.0), 2.0), 0.0);
        }

        #[test]
        fn center_64_returns_no_change() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(64.0), 2.0), 0.0);
        }

        #[test]
        fn clockwise_min_1_returns_positive_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(1.0), 2.0), 2.0);
        }

        #[test]
        fn clockwise_max_63_returns_positive_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(63.0), 2.0), 2.0);
        }

        #[test]
        fn clockwise_mid_32_returns_positive_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(32.0), 5.0), 5.0);
        }

        #[test]
        fn counter_clockwise_min_65_returns_negative_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(65.0), 2.0), -2.0);
        }

        #[test]
        fn counter_clockwise_max_127_returns_negative_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(127.0), 2.0), -2.0);
        }

        #[test]
        fn counter_clockwise_mid_96_returns_negative_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(96.0), 3.0), -3.0);
        }

        #[test]
        fn out_of_range_negative_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(-1.0), 2.0), 0.0);
        }

        #[test]
        fn out_of_range_above_127_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(128.0), 2.0), 0.0);
        }

        #[test]
        fn string_value_returns_zero() {
            assert_eq!(
                ObsDriver::encoder_value_to_delta(&json!("invalid"), 2.0),
                0.0
            );
        }

        #[test]
        fn null_value_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&Value::Null, 2.0), 0.0);
        }

        #[test]
        fn boolean_value_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(true), 2.0), 0.0);
        }

        #[test]
        fn array_value_returns_zero() {
            assert_eq!(
                ObsDriver::encoder_value_to_delta(&json!([1, 2, 3]), 2.0),
                0.0
            );
        }

        #[test]
        fn object_value_returns_zero() {
            assert_eq!(
                ObsDriver::encoder_value_to_delta(&json!({"key": "value"}), 2.0),
                0.0
            );
        }

        #[test]
        fn respects_custom_step_value() {
            // Clockwise with step=10
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(10.0), 10.0), 10.0);
            // Counter-clockwise with step=0.5
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(100.0), 0.5), -0.5);
        }
    }

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
            assert!(!determine_use_preview(target, false)); // studio_mode=false → program
            assert!(determine_use_preview(target, true)); // studio_mode=true → preview
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
