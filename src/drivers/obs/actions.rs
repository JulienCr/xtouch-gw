//! Driver trait implementation for OBS
//!
//! Handles all action execution, initialization, and lifecycle management.

use async_trait::async_trait;
use anyhow::{Result, Context, anyhow};
use serde_json::Value;
use tracing::{info, debug, warn};

use super::{Driver, ExecutionContext, IndicatorCallback};
use super::analog::shape_analog;
use super::camera::ViewMode;
use super::driver::ObsDriver;

impl ObsDriver {
    /// Resolve camera_id to (scene, source) from camera_control config.
    /// Returns None if camera_id is not found (assumes it's already scene/source format).
    fn resolve_camera_id(&self, camera_id: &str) -> Option<(String, String)> {
        let config_guard = self.camera_control_config.read();
        config_guard.as_ref()
            .and_then(|cc| cc.cameras.iter().find(|c| c.id == camera_id))
            .map(|c| (c.scene.clone(), c.source.clone()))
    }

    /// Check if PTZ is enabled for a camera by ID.
    /// Returns true by default if camera not found (legacy compatibility).
    fn is_ptz_enabled(&self, camera_id: &str) -> bool {
        let config_guard = self.camera_control_config.read();
        config_guard.as_ref()
            .and_then(|cc| cc.cameras.iter().find(|c| c.id == camera_id))
            .map(|c| c.enable_ptz)
            .unwrap_or(true) // Default: PTZ enabled for unknown cameras
    }

    /// Check if PTZ is enabled for a camera by scene name.
    /// Returns true by default if camera not found (legacy compatibility).
    fn is_ptz_enabled_by_scene(&self, scene: &str) -> bool {
        let config_guard = self.camera_control_config.read();
        config_guard.as_ref()
            .and_then(|cc| cc.cameras.iter().find(|c| c.scene == scene))
            .map(|c| c.enable_ptz)
            .unwrap_or(true) // Default: PTZ enabled for unknown cameras
    }

    /// Parse camera params with step for nudgeX, nudgeY, scaleUniform.
    /// Supports both new format [camera_id, step] and legacy [scene, source, step].
    fn parse_camera_params_with_step(&self, params: &[Value]) -> Result<(String, String, f64)> {
        match params.len() {
            2 => {
                // New format: [camera_id, step]
                let camera_id = params[0].as_str()
                    .ok_or_else(|| anyhow!("camera_id must be a string"))?;
                let step = params[1].as_f64()
                    .ok_or_else(|| anyhow!("step must be a number"))?;

                let (scene, source) = self.resolve_camera_id(camera_id)
                    .ok_or_else(|| anyhow!("Camera '{}' not found in camera_control config", camera_id))?;

                Ok((scene, source, step))
            }
            3 => {
                // Legacy format: [scene, source, step]
                let scene = params[0].as_str()
                    .ok_or_else(|| anyhow!("scene must be a string"))?.to_string();
                let source = params[1].as_str()
                    .ok_or_else(|| anyhow!("source must be a string"))?.to_string();
                let step = params[2].as_f64()
                    .ok_or_else(|| anyhow!("step must be a number"))?;
                Ok((scene, source, step))
            }
            _ => Err(anyhow!("Expected [camera_id, step] or [scene, source, step]"))
        }
    }

    /// Parse camera params for resetPosition, resetZoom.
    /// Supports both new format [camera_id] and legacy [scene, source].
    fn parse_camera_params(&self, params: &[Value]) -> Result<(String, String)> {
        match params.len() {
            1 => {
                // New format: [camera_id]
                let camera_id = params[0].as_str()
                    .ok_or_else(|| anyhow!("camera_id must be a string"))?;

                self.resolve_camera_id(camera_id)
                    .ok_or_else(|| anyhow!("Camera '{}' not found in camera_control config", camera_id))
            }
            2 => {
                // Legacy format: [scene, source]
                let scene = params[0].as_str()
                    .ok_or_else(|| anyhow!("scene must be a string"))?.to_string();
                let source = params[1].as_str()
                    .ok_or_else(|| anyhow!("source must be a string"))?.to_string();
                Ok((scene, source))
            }
            _ => Err(anyhow!("Expected [camera_id] or [scene, source]"))
        }
    }
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
                let driver_clone = self.clone_for_reconnect();
                tokio::spawn(async move {
                    driver_clone.schedule_reconnect().await;
                });
            }
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
                let driver_clone = self.clone_for_reconnect();
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
                let scene_name = params.get(0)
                    .and_then(|v| v.as_str())
                    .context("Scene name required")?;

                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?;

                // Check studio mode to determine which scene to change
                let studio_mode = *self.studio_mode.read();

                if studio_mode {
                    info!("🎬 OBS Preview scene change → '{}'", scene_name);
                    client.scenes().set_current_preview_scene(scene_name).await?;
                } else {
                    info!("🎬 OBS Program scene change → '{}'", scene_name);
                    client.scenes().set_current_program_scene(scene_name).await?;
                }

                Ok(())
            },

            "toggleStudioMode" => {
                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?;

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
                let client = guard.as_ref()
                    .context("OBS not connected")?;

                client.transitions().trigger().await?;
                Ok(())
            },

            "nudgeX" => {
                // Parse params: [camera_id, step] or [scene, source, step]
                let (scene_name, source_name, step) = self.parse_camera_params_with_step(&params)
                    .unwrap_or_else(|_| {
                        // Fallback for backward compatibility with missing step
                        let scene = params.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let source = params.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        (scene, source, 2.0)
                    });

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                // Check if input is from gamepad or encoder
                let is_gamepad = ctx.control_id.as_ref()
                    .map(|id| id.starts_with("gamepad"))
                    .unwrap_or(false);

                if is_gamepad {
                    // Gamepad analog input: velocity-based
                    if let Some(Value::Number(n)) = ctx.value {
                        if let Some(v) = n.as_f64() {
                            // Accept values slightly outside [-1.0, 1.0] due to floating point precision
                            // Clamp to valid range to avoid issues downstream
                            let clamped = v.clamp(-1.0, 1.0);

                            // Shape analog value (gamma curve only, deadzone already applied upstream)
                            let gamma = *self.analog_gamma.read();
                            let shaped = shape_analog(clamped, gamma);

                            // Calculate velocity (px per 60Hz tick)
                            let gain = *self.analog_pan_gain.read();
                            let vx = shaped * step * gain;

                            // Set analog velocity (timer will apply)
                            self.set_analog_rate(&scene_name, &source_name, Some(vx), None, None);
                        }
                    }
                } else {
                    // Encoder input: acceleration-based
                    let delta = if let Some(value) = ctx.value {
                        match value {
                            Value::Number(n) if n.is_f64() => {
                                let v = n.as_f64().unwrap();
                                if v == 0.0 || v == 64.0 {
                                    0.0
                                } else if v >= 1.0 && v <= 63.0 {
                                    step
                                } else if v >= 65.0 && v <= 127.0 {
                                    -step
                                } else {
                                    0.0
                                }
                            },
                            _ => 0.0,
                        }
                    } else {
                        step
                    };

                    if delta != 0.0 {
                        // Apply encoder acceleration
                        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
                        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
                        let final_delta = delta * accel;

                        debug!("OBS nudgeX encoder: id='{}' delta={} accel={:.2}x final={:.2}",
                            control_id, delta, accel, final_delta);

                        self.apply_delta(&scene_name, &source_name, Some(final_delta), None, None).await?;
                    }
                }
                Ok(())
            },

            "nudgeY" => {
                // Parse params: [camera_id, step] or [scene, source, step]
                let (scene_name, source_name, step) = self.parse_camera_params_with_step(&params)
                    .unwrap_or_else(|_| {
                        // Fallback for backward compatibility with missing step
                        let scene = params.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let source = params.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        (scene, source, 2.0)
                    });

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                // Check if input is from gamepad or encoder
                let is_gamepad = ctx.control_id.as_ref()
                    .map(|id| id.starts_with("gamepad"))
                    .unwrap_or(false);

                if is_gamepad {
                    // Gamepad analog input: velocity-based
                    if let Some(Value::Number(n)) = ctx.value {
                        if let Some(v) = n.as_f64() {
                            // Accept values slightly outside [-1.0, 1.0] due to floating point precision
                            // Clamp to valid range to avoid issues downstream
                            let clamped = v.clamp(-1.0, 1.0);

                            // Shape analog value (gamma curve only, deadzone already applied upstream)
                            let gamma = *self.analog_gamma.read();
                            let shaped = shape_analog(clamped, gamma);

                            // Calculate velocity (px per 60Hz tick)
                            let gain = *self.analog_pan_gain.read();
                            let vy = shaped * step * gain;

                            // Set analog velocity (timer will apply)
                            self.set_analog_rate(&scene_name, &source_name, None, Some(vy), None);
                        }
                    }
                } else {
                    // Encoder input: acceleration-based
                    let delta = if let Some(value) = ctx.value {
                        match value {
                            Value::Number(n) if n.is_f64() => {
                                let v = n.as_f64().unwrap();
                                if v == 0.0 || v == 64.0 {
                                    0.0
                                } else if v >= 1.0 && v <= 63.0 {
                                    step
                                } else if v >= 65.0 && v <= 127.0 {
                                    -step
                                } else {
                                    0.0
                                }
                            },
                            _ => 0.0,
                        }
                    } else {
                        step
                    };

                    if delta != 0.0 {
                        // Apply encoder acceleration
                        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
                        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
                        let final_delta = delta * accel;

                        debug!("OBS nudgeY encoder: id='{}' delta={} accel={:.2}x final={:.2}",
                            control_id, delta, accel, final_delta);

                        self.apply_delta(&scene_name, &source_name, None, Some(final_delta), None).await?;
                    }
                }
                Ok(())
            },

            "scaleUniform" => {
                // Parse params: [camera_id, base] or [scene, source, base]
                let (scene_name, source_name, base) = self.parse_camera_params_with_step(&params)
                    .unwrap_or_else(|_| {
                        // Fallback for backward compatibility with missing base
                        let scene = params.get(0).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let source = params.get(1).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        (scene, source, 0.02)
                    });

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                // Check if input is from gamepad or encoder
                let is_gamepad = ctx.control_id.as_ref()
                    .map(|id| id.starts_with("gamepad"))
                    .unwrap_or(false);

                if is_gamepad {
                    // Gamepad analog input: velocity-based
                    if let Some(Value::Number(n)) = ctx.value {
                        if let Some(v) = n.as_f64() {
                            // Accept values slightly outside [-1.0, 1.0] due to floating point precision
                            // Clamp to valid range to avoid issues downstream
                            let clamped = v.clamp(-1.0, 1.0);

                            // Shape analog value (gamma curve only, deadzone already applied upstream)
                            let gamma = *self.analog_gamma.read();
                            let shaped = shape_analog(clamped, gamma);

                            // Calculate velocity (scale delta per 60Hz tick)
                            let gain = *self.analog_zoom_gain.read();
                            let vs = shaped * base * gain;

                            // Set analog velocity (timer will apply)
                            self.set_analog_rate(&scene_name, &source_name, None, None, Some(vs));
                        }
                    }
                } else {
                    // Encoder input: acceleration-based
                    let delta = if let Some(value) = ctx.value {
                        match value {
                            Value::Number(n) if n.is_f64() => {
                                let v = n.as_f64().unwrap();
                                if v == 0.0 || v == 64.0 {
                                    0.0
                                } else if v >= 1.0 && v <= 63.0 {
                                    base
                                } else if v >= 65.0 && v <= 127.0 {
                                    -base
                                } else {
                                    0.0
                                }
                            },
                            _ => 0.0,
                        }
                    } else {
                        base
                    };

                    if delta != 0.0 {
                        // Apply encoder acceleration
                        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
                        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
                        let final_delta = delta * accel;

                        debug!("OBS scaleUniform encoder: id='{}' delta={} accel={:.2}x final={:.2}",
                            control_id, delta, accel, final_delta);

                        self.apply_delta(&scene_name, &source_name, None, None, Some(final_delta)).await?;
                    }
                }
                Ok(())
            },

            "resetPosition" => {
                // Parse params: [camera_id] or [scene, source]
                let (scene_name, source_name) = self.parse_camera_params(&params)
                    .context("resetPosition requires [camera_id] or [scene, source]")?;

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                info!("OBS Reset position: scene='{}' source='{}'", scene_name, source_name);

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
                let client = guard.as_ref()
                    .context("OBS client not connected")?;

                client.scene_items()
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

                debug!("OBS reset position: '{}' position=({:.1},{:.1}) alignment=CENTER",
                    self.cache_key(&scene_name, &source_name),
                    canvas_width / 2.0, canvas_height / 2.0);

                Ok(())
            },

            "resetZoom" => {
                // Parse params: [camera_id] or [scene, source]
                let (scene_name, source_name) = self.parse_camera_params(&params)
                    .context("resetZoom requires [camera_id] or [scene, source]")?;

                // Check if PTZ is enabled for this camera
                if !self.is_ptz_enabled_by_scene(&scene_name) {
                    return Ok(()); // Silently ignore PTZ commands for disabled cameras
                }

                info!("OBS Reset zoom: scene='{}' source='{}'", scene_name, source_name);

                // Stop any analog motion on zoom axis
                self.set_analog_rate(&scene_name, &source_name, None, None, Some(0.0));

                // Resolve item ID
                let item_id = self.resolve_item_id(&scene_name, &source_name).await?;

                // Read current transform from OBS (not cache!) to determine type
                let current = self.read_transform(&scene_name, item_id).await?;

                // Detect if this source uses bounds-based or scale-based transform
                let is_bounds_based = if let (Some(bw), Some(bh)) = (current.bounds_width, current.bounds_height) {
                    bw > 0.0 && bh > 0.0
                } else {
                    false
                };

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

                    info!("OBS Reset zoom (bounds): {}x{}", canvas_width as u32, canvas_height as u32);
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
                let client = guard.as_ref()
                    .context("OBS client not connected")?;

                client.scene_items()
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
                let camera_id = params.get(0)
                    .and_then(|v| v.as_str())
                    .context("Camera ID required")?;
                
                let (view_mode, camera_scene_opt, split_scene_opt) = {
                    let view_mode = self.camera_control_state.read().current_view_mode;
                    let config_guard = self.camera_control_config.read();
                    let config = config_guard.as_ref()
                        .context("Camera control not configured")?;
                    
                    // Find camera config
                    let camera = config.cameras.iter()
                        .find(|c| c.id == camera_id)
                        .with_context(|| format!("Camera '{}' not found", camera_id))?;
                    
                    match view_mode {
                        ViewMode::Full => {
                            (view_mode, Some(camera.scene.clone()), None)
                        },
                        ViewMode::SplitLeft => {
                            (view_mode, None, Some(config.splits.left.clone()))
                        },
                        ViewMode::SplitRight => {
                            (view_mode, None, Some(config.splits.right.clone()))
                        },
                    }
                };
                
                match view_mode {
                    ViewMode::Full => {
                        let camera_scene = camera_scene_opt.unwrap();
                        info!("🎬 OBS: Select camera '{}' (FULL mode) → scene '{}'", camera_id, camera_scene);
                        
                        let guard = self.client.read().await;
                        let client = guard.as_ref()
                            .context("OBS not connected")?;
                        
                        let studio_mode = *self.studio_mode.read();
                        
                        if studio_mode {
                            client.scenes().set_current_preview_scene(&camera_scene).await?;
                        } else {
                            client.scenes().set_current_program_scene(&camera_scene).await?;
                        }
                        
                        // Update last_camera
                        self.camera_control_state.write().last_camera = camera_id.to_string();
                    },
                    ViewMode::SplitLeft | ViewMode::SplitRight => {
                        let split_scene = split_scene_opt.unwrap();
                        info!("🎬 OBS: Select camera '{}' (SPLIT mode) in '{}'", camera_id, split_scene);
                        
                        self.set_split_camera(&split_scene, camera_id).await?;
                        
                        // Update last_camera
                        self.camera_control_state.write().last_camera = camera_id.to_string();
                    },
                }
                
                Ok(())
            },

            "enterSplit" => {
                let side = params.get(0)
                    .and_then(|v| v.as_str())
                    .context("Side required ('left' or 'right')")?;
                
                let (split_scene, new_mode, last_camera) = {
                    let config_guard = self.camera_control_config.read();
                    let config = config_guard.as_ref()
                        .context("Camera control not configured")?;
                    
                    // Determine split scene
                    let (split_scene, new_mode) = match side {
                        "left" => (config.splits.left.clone(), ViewMode::SplitLeft),
                        "right" => (config.splits.right.clone(), ViewMode::SplitRight),
                        _ => return Err(anyhow!("Invalid side '{}', must be 'left' or 'right'", side)),
                    };
                    
                    let last_camera = self.camera_control_state.read().last_camera.clone();
                    
                    // If last_camera is empty, use first camera
                    let last_camera = if last_camera.is_empty() {
                        config.cameras.first()
                            .map(|c| c.id.clone())
                            .unwrap_or_else(|| "Main".to_string())
                    } else {
                        last_camera
                    };
                    
                    (split_scene, new_mode, last_camera)
                };
                
                info!("🎬 OBS: Enter split '{}' → scene '{}'", side, split_scene);
                
                // Switch to split scene
                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?;
                
                let studio_mode = *self.studio_mode.read();
                
                if studio_mode {
                    client.scenes().set_current_preview_scene(&split_scene).await?;
                } else {
                    client.scenes().set_current_program_scene(&split_scene).await?;
                }

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
                    let config = config_guard.as_ref()
                        .context("Camera control not configured")?;
                    
                    // Find last camera scene
                    let last_camera = self.camera_control_state.read().last_camera.clone();
                    let camera = config.cameras.iter()
                        .find(|c| c.id == last_camera)
                        .or_else(|| config.cameras.first())
                        .context("No cameras configured")?;
                    
                    (last_camera, camera.scene.clone())
                };
                
                info!("🎬 OBS: Exit split → camera '{}' scene '{}'", last_camera, camera_scene);
                
                // Switch to full scene
                let guard = self.client.read().await;
                let client = guard.as_ref()
                    .context("OBS not connected")?;
                
                let studio_mode = *self.studio_mode.read();
                
                if studio_mode {
                    client.scenes().set_current_preview_scene(&camera_scene).await?;
                } else {
                    client.scenes().set_current_program_scene(&camera_scene).await?;
                }
                
                // Update state
                self.camera_control_state.write().current_view_mode = ViewMode::Full;
                
                Ok(())
            },

            _ => {
                warn!("Unknown OBS action: {}", action);
                Ok(())
            }
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
            emit("obs.currentProgramScene".to_string(), Value::String(program_scene.clone()));
            emit("obs.currentPreviewScene".to_string(), Value::String(preview_scene.clone()));

            // Emit composite selectedScene
            let selected = if studio_mode { preview_scene } else { program_scene };
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

