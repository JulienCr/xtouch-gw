//! Camera control for split view management
//!
//! Handles camera selection and split view modes (left/right/full).

use anyhow::{Result, Context};
use tracing::{debug, info};

use super::driver::ObsDriver;

/// View mode for camera control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ViewMode {
    Full,
    SplitLeft,
    SplitRight,
}

/// Camera control state (shared between all gamepads)
#[derive(Debug, Clone)]
pub(super) struct CameraControlState {
    pub(super) current_view_mode: ViewMode,
    pub(super) last_camera: String,
}

impl Default for CameraControlState {
    fn default() -> Self {
        Self {
            current_view_mode: ViewMode::Full,
            last_camera: String::new(),
        }
    }
}

impl ObsDriver {
    /// Detect ViewMode from scene name using camera_control_config
    ///
    /// Returns:
    /// - Some(ViewMode::SplitLeft) if scene matches left split
    /// - Some(ViewMode::SplitRight) if scene matches right split
    /// - Some(ViewMode::Full) if scene matches a camera scene
    /// - None if camera control not configured or scene doesn't match
    pub(super) fn detect_view_mode_from_scene(&self, scene_name: &str) -> Option<ViewMode> {
        let config_guard = self.camera_control_config.read();
        let config = config_guard.as_ref()?;

        // Check if scene is a split scene
        if scene_name == config.splits.left {
            return Some(ViewMode::SplitLeft);
        }
        if scene_name == config.splits.right {
            return Some(ViewMode::SplitRight);
        }

        // Check if scene is a camera scene
        for camera in &config.cameras {
            if scene_name == camera.scene {
                return Some(ViewMode::Full);
            }
        }

        // Unknown scene (e.g., "BRB Screen", graphics, etc.)
        None
    }

    /// Update camera control ViewMode state based on scene name
    ///
    /// This should be called whenever the active scene changes (at connection
    /// or via scene change events). It synchronizes the internal ViewMode with
    /// the actual OBS scene.
    pub(super) fn update_view_mode_from_scene(&self, scene_name: &str) {
        if let Some(view_mode) = self.detect_view_mode_from_scene(scene_name) {
            let mut state = self.camera_control_state.write();
            let old_mode = state.current_view_mode;
            state.current_view_mode = view_mode;

            if old_mode != view_mode {
                debug!("ViewMode synced from scene '{}': {:?} → {:?}", scene_name, old_mode, view_mode);
            }
        }
        // If None, preserve current ViewMode (non-camera scene like "BRB Screen")
    }

    /// Helper: Set scene item enabled/disabled
    pub(super) async fn set_scene_item_enabled(&self, scene_name: &str, source_name: &str, enabled: bool) -> Result<()> {
        let item_id = self.resolve_item_id(scene_name, source_name).await?;
        
        let guard = self.client.read().await;
        let client = guard.as_ref()
            .context("OBS client not connected")?
            .clone();
        
        client.scene_items()
            .set_enabled(obws::requests::scene_items::SetEnabled {
                scene: scene_name,
                item_id,
                enabled,
            })
            .await
            .with_context(|| format!("Failed to set item '{}' enabled={} in scene '{}'", source_name, enabled, scene_name))?;
        
        debug!("OBS: Set '{}' in '{}' enabled={}", source_name, scene_name, enabled);
        Ok(())
    }

    /// Helper: Set split camera (hide all, show one)
    pub(super) async fn set_split_camera(&self, split_scene: &str, camera_id: &str) -> Result<()> {
        let (cameras, target_split_source) = {
            let config_guard = self.camera_control_config.read();
            let config = config_guard.as_ref()
                .context("Camera control not configured")?;
            
            // Get all camera split sources
            let cameras: Vec<String> = config.cameras.iter()
                .map(|c| c.split_source.clone())
                .collect();
            
            // Find target camera
            let target_camera = config.cameras.iter()
                .find(|c| c.id == camera_id)
                .with_context(|| format!("Camera '{}' not found in config", camera_id))?;
            
            (cameras, target_camera.split_source.clone())
        };
        
        // Hide all SPLIT CAM sources
        for camera_source in &cameras {
            self.set_scene_item_enabled(split_scene, camera_source, false).await?;
        }
        
        // Show the target camera
        self.set_scene_item_enabled(split_scene, &target_split_source, true).await?;
        
        info!("🎬 OBS: Set split camera '{}' in '{}'", camera_id, split_scene);
        Ok(())
    }
}

