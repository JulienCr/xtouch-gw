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

