//! `/api/page` — read and set the active page index.
//!
//! Used by the editor to mirror page-tab selection with the X-Touch's
//! active page. The router emits a `PageChanged` live event when this
//! changes (regardless of source), which the editor listens to.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use super::EditorState;

#[derive(Debug, Serialize)]
pub struct ActivePage {
    pub index: usize,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SetActivePageRequest {
    pub index: usize,
}

/// `GET /api/page` → `{ index, name }`.
pub async fn active(State(state): State<Arc<EditorState>>) -> Response {
    let Some(reader) = state.active_page_reader.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "page reader not wired").into_response();
    };
    match reader().await {
        Some((index, name)) => Json(ActivePage { index, name }).into_response(),
        None => (StatusCode::NOT_FOUND, "no active page").into_response(),
    }
}

/// `POST /api/page` with `{ "index": N }` → switches active page.
pub async fn set_active(
    State(state): State<Arc<EditorState>>,
    Json(req): Json<SetActivePageRequest>,
) -> Response {
    let Some(setter) = state.active_page_setter.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "page setter not wired").into_response();
    };
    match setter(req.index).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}
