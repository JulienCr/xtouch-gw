//! Camera selection actions for OBS
//!
//! Handles `selectCamera` action execution and PTZ target context.

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::{debug, info};

use super::camera::ViewMode;
use super::driver::ObsDriver;
use super::ExecutionContext;

/// Extract gamepad slot from control_id (e.g., "gamepad1" from "gamepad1.btn.a")
pub(super) fn extract_gamepad_slot(control_id: &str) -> String {
    control_id
        .split('.')
        .next()
        .unwrap_or("gamepad1")
        .to_string()
}

/// Context for setting PTZ target from gamepad controls
pub(super) struct PtzTargetContext<'a> {
    control_id: Option<&'a str>,
    camera_targets: Option<&'a std::sync::Arc<crate::router::CameraTargetState>>,
}

impl<'a> PtzTargetContext<'a> {
    /// Create context from ExecutionContext
    pub(super) fn from_ctx(ctx: &'a ExecutionContext) -> Self {
        Self {
            control_id: ctx.control_id.as_deref(),
            camera_targets: ctx.camera_targets.as_ref(),
        }
    }

    /// Check if this is a gamepad control
    pub(super) fn is_gamepad(&self) -> bool {
        self.control_id
            .map(|id| id.starts_with("gamepad"))
            .unwrap_or(false)
    }

    /// Get gamepad slot from control_id
    pub(super) fn gamepad_slot(&self) -> String {
        self.control_id
            .map(extract_gamepad_slot)
            .unwrap_or_else(|| "gamepad1".to_string())
    }

    /// Set PTZ target for gamepad if applicable. Returns true if target was set.
    pub(super) fn set_ptz_target(
        &self,
        camera_id: &str,
        ptz_enabled: bool,
        context_label: &str,
    ) -> bool {
        if !self.is_gamepad() || !ptz_enabled {
            return false;
        }

        let Some(camera_targets) = self.camera_targets else {
            return false;
        };

        let slot = self.gamepad_slot();
        match camera_targets.set_target(&slot, camera_id) {
            Ok(()) => {
                tracing::info!(
                    "PTZ target set ({}): {} -> {}",
                    context_label,
                    slot,
                    camera_id
                );
                true
            },
            Err(e) => {
                tracing::warn!("Failed to set PTZ target on {}: {}", context_label, e);
                false
            },
        }
    }
}

impl ObsDriver {
    /// Execute the `selectCamera` action.
    ///
    /// Selects a camera in full or split mode, handling preview/program routing
    /// and PTZ target assignment for gamepad controls.
    pub(super) async fn execute_select_camera(
        &self,
        params: &[Value],
        ctx: &ExecutionContext,
    ) -> Result<()> {
        let camera_id = params
            .first()
            .and_then(|v| v.as_str())
            .context("Camera ID required")?;

        // Parse optional target parameter: "preview", "program", or absent (check modifier)
        let explicit_target = params.get(1).and_then(|v| v.as_str());
        let ptz_ctx = PtzTargetContext::from_ctx(ctx);

        // Check if PTZ modifier is held for this gamepad slot
        let modifier_held = ctx
            .camera_targets
            .as_ref()
            .map(|ct| ct.is_ptz_modifier_held(&ptz_ctx.gamepad_slot()))
            .unwrap_or(false);

        // Get view mode and resolve scene names from config
        let view_mode = self.camera_control_state.read().current_view_mode;
        let (camera_scene, split_scene, ptz_enabled) = {
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

            (camera.scene.clone(), split, camera.enable_ptz)
        };

        match view_mode {
            ViewMode::Full => {
                self.execute_select_camera_full(
                    camera_id,
                    &camera_scene,
                    explicit_target,
                    &ptz_ctx,
                    modifier_held,
                    ptz_enabled,
                )
                .await?;
            },
            ViewMode::SplitLeft | ViewMode::SplitRight => {
                let split_scene = split_scene.context("BUG: split_scene missing for split mode")?;

                if let Some(target) = explicit_target {
                    debug!(
                        "Ignoring target '{}' in SPLIT mode (split modifies sources, not scenes)",
                        target
                    );
                }

                info!(
                    "OBS: Select camera '{}' (SPLIT mode) in '{}'",
                    camera_id, split_scene
                );
                self.set_split_camera(&split_scene, camera_id).await?;

                // Set PTZ target automatically in SPLIT mode (no modifier needed)
                ptz_ctx.set_ptz_target(camera_id, ptz_enabled, "SPLIT");
            },
        }

        // Update last_camera for all modes
        self.camera_control_state.write().last_camera = camera_id.to_string();

        Ok(())
    }

    /// Execute selectCamera in Full view mode.
    ///
    /// Handles preview/program routing based on explicit target, gamepad modifier,
    /// or legacy studio mode behavior.
    async fn execute_select_camera_full(
        &self,
        camera_id: &str,
        camera_scene: &str,
        explicit_target: Option<&str>,
        ptz_ctx: &PtzTargetContext<'_>,
        modifier_held: bool,
        ptz_enabled: bool,
    ) -> Result<()> {
        let guard = self.get_connected_client().await?;
        let client = guard
            .as_ref()
            .expect("invariant: get_connected_client ensures Some");

        // Determine whether to use preview or program
        // Priority: explicit_target > gamepad modifier logic > legacy behavior
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
            None if ptz_ctx.is_gamepad() && modifier_held => {
                // Gamepad + PTZ modifier held - force preview mode
                if !*self.studio_mode.read() {
                    info!("Enabling studio mode for PTZ modifier preview");
                    client.ui().set_studio_mode_enabled(true).await?;
                    *self.studio_mode.write() = true;
                }
                true
            },
            None if ptz_ctx.is_gamepad() => {
                // Gamepad without modifier - force program mode
                false
            },
            _ => *self.studio_mode.read(), // Legacy behavior for non-gamepad
        };

        let target_name = if use_preview { "preview" } else { "program" };
        info!(
            "OBS: Select camera '{}' (FULL mode) -> {} '{}'",
            camera_id, target_name, camera_scene
        );

        if use_preview {
            client
                .scenes()
                .set_current_preview_scene(camera_scene)
                .await?;

            // If modifier held and PTZ enabled, also set PTZ target
            if modifier_held {
                ptz_ctx.set_ptz_target(camera_id, ptz_enabled, "FULL+modifier");
            }
        } else {
            client
                .scenes()
                .set_current_program_scene(camera_scene)
                .await?;
        }

        Ok(())
    }
}
