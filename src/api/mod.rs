//! REST API for Stream Deck integration
//!
//! Provides HTTP endpoints and WebSocket for controlling dynamic camera targets.
//! Default port: 8125

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Default API port
pub const DEFAULT_API_PORT: u16 = 8125;

/// Shared state for API handlers
pub struct ApiState {
    /// Camera target state manager
    pub camera_targets: Arc<crate::router::CameraTargetState>,
    /// Available cameras from config (id -> scene name)
    pub available_cameras: Arc<parking_lot::RwLock<Vec<CameraInfo>>>,
    /// Gamepad slot configurations
    pub gamepad_slots: Arc<parking_lot::RwLock<Vec<GamepadSlotInfo>>>,
    /// Broadcast channel for camera updates
    pub update_tx: broadcast::Sender<CameraUpdate>,
}

/// Camera update notification (sent via WebSocket)
#[derive(Debug, Clone, Serialize)]
pub struct CameraUpdate {
    pub gamepad_slot: String,
    pub camera_id: String,
    pub timestamp: u64,
}

/// Request body for setting camera target
#[derive(Debug, Deserialize)]
pub struct SetCameraRequest {
    pub camera_id: String,
}

/// Response for get camera target
#[derive(Debug, Serialize)]
pub struct GetCameraResponse {
    pub camera_id: Option<String>,
    pub mode: String,
}

/// Information about a camera
#[derive(Debug, Clone, Serialize)]
pub struct CameraInfo {
    pub id: String,
    pub scene: String,
    pub source: String,
    pub split_source: String,
    pub enable_ptz: bool,
}

/// Information about a gamepad slot
#[derive(Debug, Clone, Serialize)]
pub struct GamepadSlotInfo {
    pub slot: String,
    pub product_match: String,
    pub camera_target_mode: String,
    pub current_camera: Option<String>,
}

/// API error response
#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(self)).into_response()
    }
}

/// Build the API router
pub fn build_router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route(
            "/api/gamepad/:slot/camera",
            get(get_camera_target).put(set_camera_target),
        )
        .route("/api/gamepads", get(list_gamepads))
        .route("/api/cameras", get(list_cameras))
        .route("/api/ws/camera-updates", get(camera_updates_ws))
        .route("/api/health", get(health_check))
        .with_state(state)
}

/// GET /api/gamepad/:slot/camera - Get current camera target for a gamepad
async fn get_camera_target(
    Path(slot): Path<String>,
    State(state): State<Arc<ApiState>>,
) -> Json<GetCameraResponse> {
    let camera_id = state.camera_targets.get_target(&slot);

    // Determine mode from gamepad slots
    let mode = state
        .gamepad_slots
        .read()
        .iter()
        .find(|g| g.slot == slot)
        .map(|g| g.camera_target_mode.clone())
        .unwrap_or_else(|| "unknown".to_string());

    Json(GetCameraResponse { camera_id, mode })
}

/// PUT /api/gamepad/:slot/camera - Set camera target for a gamepad
async fn set_camera_target(
    Path(slot): Path<String>,
    State(state): State<Arc<ApiState>>,
    Json(req): Json<SetCameraRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate camera_id exists
    let camera_exists = state
        .available_cameras
        .read()
        .iter()
        .any(|c| c.id == req.camera_id);

    if !camera_exists {
        return Err(ApiError {
            error: format!(
                "Invalid camera_id: '{}'. Use GET /api/cameras to see available cameras.",
                req.camera_id
            ),
        });
    }

    // Set the target
    if let Err(e) = state.camera_targets.set_target(&slot, &req.camera_id) {
        error!("Failed to set camera target: {}", e);
        return Err(ApiError {
            error: format!("Failed to set camera target: {}", e),
        });
    }

    // Broadcast update to WebSocket subscribers
    let update = CameraUpdate {
        gamepad_slot: slot.clone(),
        camera_id: req.camera_id.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    };

    // Best-effort broadcast (ignore if no subscribers)
    let _ = state.update_tx.send(update);

    info!("Camera target set: {} -> {}", slot, req.camera_id);

    Ok(Json(serde_json::json!({
        "ok": true,
        "camera_id": req.camera_id
    })))
}

/// GET /api/gamepads - List all gamepad slots with their current camera targets
async fn list_gamepads(State(state): State<Arc<ApiState>>) -> Json<Vec<GamepadSlotInfo>> {
    let mut slots = state.gamepad_slots.read().clone();

    // Update current camera targets
    for slot in &mut slots {
        slot.current_camera = state.camera_targets.get_target(&slot.slot);
    }

    Json(slots)
}

/// GET /api/cameras - List available cameras
async fn list_cameras(State(state): State<Arc<ApiState>>) -> Json<Vec<CameraInfo>> {
    Json(state.available_cameras.read().clone())
}

/// GET /api/ws/camera-updates - WebSocket for push notifications
async fn camera_updates_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApiState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state.update_tx.subscribe()))
}

/// Handle WebSocket connection for camera updates
async fn handle_websocket(mut socket: WebSocket, mut rx: broadcast::Receiver<CameraUpdate>) {
    debug!("WebSocket client connected for camera updates");

    // Send initial state
    // (Could send current targets here if needed)

    loop {
        tokio::select! {
            // Forward updates to WebSocket
            result = rx.recv() => {
                match result {
                    Ok(update) => {
                        let msg = serde_json::to_string(&update).unwrap();
                        if socket.send(Message::Text(msg.into())).await.is_err() {
                            debug!("WebSocket client disconnected");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Broadcast channel closed");
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WebSocket client lagged by {} messages", n);
                    }
                }
            }
            // Handle incoming messages (for future use, e.g., ping/pong)
            result = socket.recv() => {
                match result {
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("WebSocket client closed connection");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Ignore other messages
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket error: {}", e);
                        break;
                    }
                }
            }
        }
    }
}

/// GET /api/health - Health check endpoint
async fn health_check() -> &'static str {
    "ok"
}

/// Start the API server
pub async fn start_server(state: Arc<ApiState>, port: u16) -> Result<()> {
    let router = build_router(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    info!("Starting Stream Deck API server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind API server")?;

    axum::serve(listener, router)
        .await
        .context("API server error")?;

    Ok(())
}
