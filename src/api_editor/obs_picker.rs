//! OBS picker endpoints for the editor.
//!
//! The editor calls these on demand to populate scene / source / input
//! dropdowns. Backed by an injected `dyn ObsPickerSource` so api_editor
//! does not depend on the concrete `ObsDriver` type — useful both for
//! the binary (which wires the real driver in) and for tests (which
//! can inject a mock).

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use super::EditorState;

/// Read-only accessor over OBS used by the picker endpoints.
///
/// Implementations should be cheap to call (no caching is performed on the
/// editor side; the UI re-fetches when the user clicks "refresh").
#[async_trait]
pub trait ObsPickerSource: Send + Sync {
    async fn list_scenes(&self) -> anyhow::Result<Vec<String>>;
    async fn list_scene_items(&self, scene: &str) -> anyhow::Result<Vec<(String, String)>>;
    async fn list_inputs(&self) -> anyhow::Result<Vec<(String, String)>>;
}

/// Shared accessor type stored in `EditorState`.
pub type ObsPickerSourceArc = Arc<dyn ObsPickerSource>;

#[derive(Serialize)]
pub struct SceneRef {
    pub name: String,
}

#[derive(Serialize)]
pub struct NamedKind {
    pub name: String,
    pub kind: String,
}

#[derive(Serialize)]
pub struct PickerError {
    pub error: &'static str,
}

fn obs_unavailable() -> (StatusCode, Json<PickerError>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(PickerError {
            error: "obs_not_connected",
        }),
    )
}

fn obs_failed() -> (StatusCode, Json<PickerError>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(PickerError {
            error: "obs_request_failed",
        }),
    )
}

/// `GET /api/obs/scenes`
pub async fn scenes(
    State(state): State<Arc<EditorState>>,
) -> Result<Json<Vec<SceneRef>>, (StatusCode, Json<PickerError>)> {
    let Some(obs) = state.obs.as_ref() else {
        return Err(obs_unavailable());
    };
    match obs.list_scenes().await {
        Ok(names) => Ok(Json(
            names.into_iter().map(|name| SceneRef { name }).collect(),
        )),
        Err(e) => {
            tracing::warn!("obs picker: list_scenes failed: {}", e);
            Err(obs_failed())
        },
    }
}

/// `GET /api/obs/scenes/:scene/sources`
pub async fn scene_sources(
    State(state): State<Arc<EditorState>>,
    Path(scene): Path<String>,
) -> Result<Json<Vec<NamedKind>>, (StatusCode, Json<PickerError>)> {
    let Some(obs) = state.obs.as_ref() else {
        return Err(obs_unavailable());
    };
    match obs.list_scene_items(&scene).await {
        Ok(items) => Ok(Json(
            items
                .into_iter()
                .map(|(name, kind)| NamedKind { name, kind })
                .collect(),
        )),
        Err(e) => {
            tracing::warn!("obs picker: list_scene_items failed: {}", e);
            Err(obs_failed())
        },
    }
}

/// `GET /api/obs/inputs`
pub async fn inputs(
    State(state): State<Arc<EditorState>>,
) -> Result<Json<Vec<NamedKind>>, (StatusCode, Json<PickerError>)> {
    let Some(obs) = state.obs.as_ref() else {
        return Err(obs_unavailable());
    };
    match obs.list_inputs().await {
        Ok(items) => Ok(Json(
            items
                .into_iter()
                .map(|(name, kind)| NamedKind { name, kind })
                .collect(),
        )),
        Err(e) => {
            tracing::warn!("obs picker: list_inputs failed: {}", e);
            Err(obs_failed())
        },
    }
}
