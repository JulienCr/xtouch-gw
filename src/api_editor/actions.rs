//! Driver action catalog endpoints.
//!
//! Read-only metadata: lists known drivers and their declared actions so the
//! editor UI can build typed forms instead of free-text. The catalogs are
//! snapshotted at startup into a static map (keyed by driver name) so the
//! editor never has to await a live driver to render its form palette.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;

use super::action_catalog::ActionDescriptor;
use super::EditorState;

/// Snapshot of all drivers' action catalogs, keyed by driver name.
pub type DriverCatalogs = Arc<HashMap<String, Vec<ActionDescriptor>>>;

#[derive(Serialize)]
pub struct DriverRef {
    pub name: String,
}

/// `GET /api/drivers`
pub async fn list_drivers(State(state): State<Arc<EditorState>>) -> Json<Vec<DriverRef>> {
    let mut names: Vec<String> = state.drivers.keys().cloned().collect();
    names.sort();
    Json(names.into_iter().map(|name| DriverRef { name }).collect())
}

/// `GET /api/drivers/:name/actions`
pub async fn driver_actions(
    State(state): State<Arc<EditorState>>,
    Path(name): Path<String>,
) -> Result<Json<Vec<ActionDescriptor>>, StatusCode> {
    state
        .drivers
        .get(&name)
        .map(|cat| Json(cat.clone()))
        .ok_or(StatusCode::NOT_FOUND)
}
