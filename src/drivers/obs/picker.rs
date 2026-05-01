//! Picker-source implementation: thin async accessors over the live OBS
//! WebSocket client used by the editor's `/api/obs/*` endpoints.
//!
//! These methods hold the client RwLock for the duration of one obws round
//! trip — which is fine because they are only invoked on demand from the
//! editor, never from the hot path.

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::driver::ObsDriver;

impl ObsDriver {
    /// List all scene names known to OBS.
    pub async fn list_scenes(&self) -> Result<Vec<String>> {
        let guard = self.get_connected_client().await?;
        let client = guard
            .as_ref()
            .context("BUG: get_connected_client returned None")?;
        let scenes = client
            .scenes()
            .list()
            .await
            .context("OBS scenes().list()")?;
        Ok(scenes.scenes.into_iter().map(|s| s.name).collect())
    }

    /// List the items (sources) of one scene as `(name, kind)` tuples.
    pub async fn list_scene_items(&self, scene: &str) -> Result<Vec<(String, String)>> {
        let guard = self.get_connected_client().await?;
        let client = guard
            .as_ref()
            .context("BUG: get_connected_client returned None")?;
        let items = client
            .scene_items()
            .list(scene)
            .await
            .with_context(|| format!("OBS scene_items().list({})", scene))?;
        Ok(items
            .into_iter()
            .map(|item| (item.source_name, item.input_kind.unwrap_or_default()))
            .collect())
    }

    /// List all input devices as `(name, kind)` tuples.
    pub async fn list_inputs(&self) -> Result<Vec<(String, String)>> {
        let guard = self.get_connected_client().await?;
        let client = guard
            .as_ref()
            .context("BUG: get_connected_client returned None")?;
        let inputs = client
            .inputs()
            .list(None)
            .await
            .context("OBS inputs().list(None)")?;
        Ok(inputs.into_iter().map(|i| (i.name, i.kind)).collect())
    }
}

#[async_trait]
impl crate::api_editor::ObsPickerSource for ObsDriver {
    async fn list_scenes(&self) -> Result<Vec<String>> {
        ObsDriver::list_scenes(self).await
    }
    async fn list_scene_items(&self, scene: &str) -> Result<Vec<(String, String)>> {
        ObsDriver::list_scene_items(self, scene).await
    }
    async fn list_inputs(&self) -> Result<Vec<(String, String)>> {
        ObsDriver::list_inputs(self).await
    }
}
