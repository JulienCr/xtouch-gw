//! Profile CRUD + history endpoints.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use super::EditorState;
use crate::config::profiles::{ProfileError, ProfileMeta, Snapshot};

/// Map a `ProfileError` to `(StatusCode, Json<error_body>)`.
fn err_response(err: ProfileError) -> Response {
    use ProfileError::*;
    let (status, body) = match &err {
        InvalidName(name) => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "invalid_name", "name": name }),
        ),
        NotFound(name) => (
            StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "not_found", "name": name }),
        ),
        AlreadyExists(name) => (
            StatusCode::CONFLICT,
            serde_json::json!({ "error": "already_exists", "name": name }),
        ),
        Active(name) => (
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": "active",
                "name": name,
                "message": "profile is active and cannot be modified that way",
            }),
        ),
        ConflictingWrite => (
            StatusCode::CONFLICT,
            serde_json::json!({ "error": "conflicting_write" }),
        ),
        Io(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "io", "message": e.to_string() }),
        ),
        Yaml(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "yaml", "message": e.to_string() }),
        ),
        Other(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": "other", "message": e.to_string() }),
        ),
    };
    (status, Json(body)).into_response()
}

/// Augment a `ConflictingWrite` response with the current server hash + body.
fn conflict_with_current(state: &EditorState, name: &str) -> Response {
    match state.profiles.read(name) {
        Ok((body, meta)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "conflicting_write",
                "current_hash": meta.content_hash,
                "current_body": body,
            })),
        )
            .into_response(),
        Err(_) => err_response(ProfileError::ConflictingWrite),
    }
}

// ---------------- list / read ----------------

pub async fn list(State(s): State<Arc<EditorState>>) -> Response {
    match s.profiles.list() {
        Ok(metas) => Json(metas).into_response(),
        Err(e) => err_response(e),
    }
}

#[derive(Serialize)]
pub struct ReadResponse {
    pub meta: ProfileMeta,
    pub body: String,
}

pub async fn read(Path(name): Path<String>, State(s): State<Arc<EditorState>>) -> Response {
    match s.profiles.read(&name) {
        Ok((body, meta)) => Json(ReadResponse { meta, body }).into_response(),
        Err(e) => err_response(e),
    }
}

// ---------------- create ----------------

#[derive(Deserialize)]
pub struct CreateRequest {
    pub name: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

pub async fn create(State(s): State<Arc<EditorState>>, Json(req): Json<CreateRequest>) -> Response {
    let result = if let Some(src) = req.source.as_deref() {
        s.profiles.duplicate(src, &req.name)
    } else {
        let body = req.body.unwrap_or_default();
        s.profiles.create(&req.name, &body)
    };
    match result {
        Ok(meta) => (StatusCode::CREATED, Json(meta)).into_response(),
        Err(e) => err_response(e),
    }
}

// ---------------- save (PUT) ----------------

#[derive(Deserialize)]
pub struct SaveRequest {
    pub body: String,
    #[serde(default)]
    pub expected_hash: Option<String>,
}

pub async fn save(
    Path(name): Path<String>,
    State(s): State<Arc<EditorState>>,
    Json(req): Json<SaveRequest>,
) -> Response {
    match s
        .profiles
        .write(&name, &req.body, req.expected_hash.as_deref())
    {
        Ok(meta) => Json(meta).into_response(),
        Err(ProfileError::ConflictingWrite) => conflict_with_current(&s, &name),
        Err(e) => err_response(e),
    }
}

// ---------------- delete ----------------

pub async fn delete_(Path(name): Path<String>, State(s): State<Arc<EditorState>>) -> Response {
    match s.profiles.delete(&name) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err_response(e),
    }
}

// ---------------- duplicate / rename / activate ----------------

#[derive(Deserialize)]
pub struct NewNameRequest {
    pub new_name: String,
}

pub async fn duplicate(
    Path(name): Path<String>,
    State(s): State<Arc<EditorState>>,
    Json(req): Json<NewNameRequest>,
) -> Response {
    match s.profiles.duplicate(&name, &req.new_name) {
        Ok(meta) => (StatusCode::CREATED, Json(meta)).into_response(),
        Err(e) => err_response(e),
    }
}

pub async fn rename(
    Path(name): Path<String>,
    State(s): State<Arc<EditorState>>,
    Json(req): Json<NewNameRequest>,
) -> Response {
    match s.profiles.rename(&name, &req.new_name) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => err_response(e),
    }
}

pub async fn activate(Path(name): Path<String>, State(s): State<Arc<EditorState>>) -> Response {
    match s.profiles.set_active(&name) {
        Ok(()) => Json(serde_json::json!({ "ok": true, "name": name })).into_response(),
        Err(e) => err_response(e),
    }
}

#[derive(Serialize)]
pub struct ActiveResponse {
    pub name: String,
}

pub async fn active(State(s): State<Arc<EditorState>>) -> Response {
    match s.profiles.active() {
        Ok(name) => Json(ActiveResponse { name }).into_response(),
        Err(e) => err_response(e),
    }
}

// ---------------- history ----------------

pub async fn history(Path(name): Path<String>, State(s): State<Arc<EditorState>>) -> Response {
    match s.profiles.list_history(&name) {
        Ok(snaps) => Json::<Vec<Snapshot>>(snaps).into_response(),
        Err(e) => err_response(e),
    }
}

#[derive(Serialize)]
pub struct HistoryReadResponse {
    pub timestamp: String,
    pub body: String,
}

pub async fn history_read(
    Path((name, timestamp)): Path<(String, String)>,
    State(s): State<Arc<EditorState>>,
) -> Response {
    match s.profiles.read_snapshot(&name, &timestamp) {
        Ok(body) => Json(HistoryReadResponse { timestamp, body }).into_response(),
        Err(e) => err_response(e),
    }
}

pub async fn history_restore(
    Path((name, timestamp)): Path<(String, String)>,
    State(s): State<Arc<EditorState>>,
) -> Response {
    match s.profiles.restore_snapshot(&name, &timestamp) {
        Ok(meta) => Json(meta).into_response(),
        Err(e) => err_response(e),
    }
}
