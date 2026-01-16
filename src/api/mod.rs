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
    routing::{get, post},
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
    /// Broadcast channel for camera state messages
    pub update_tx: broadcast::Sender<CameraStateMessage>,
    /// Current camera on air (by camera ID, derived from OBS program scene)
    pub current_on_air_camera: Arc<parking_lot::RwLock<Option<String>>>,
    /// OBS driver for transform operations
    pub obs_driver: Option<Arc<crate::drivers::ObsDriver>>,
}

/// WebSocket message types for camera state updates
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CameraStateMessage {
    /// Full state snapshot (sent on connect)
    Snapshot {
        gamepads: Vec<GamepadSlotInfo>,
        cameras: Vec<CameraInfo>,
        on_air_camera: Option<String>,
        timestamp: u64,
    },
    /// Camera target changed for a gamepad
    TargetChanged {
        gamepad_slot: String,
        camera_id: String,
        timestamp: u64,
    },
    /// OBS program scene changed (camera went on air)
    OnAirChanged {
        camera_id: String,
        scene_name: String,
        timestamp: u64,
    },
}

/// Request body for setting camera target
#[derive(Debug, Deserialize)]
pub struct SetCameraRequest {
    pub camera_id: String,
}

/// Reset mode for camera transform reset
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResetMode {
    /// Reset only position (center on canvas)
    Position,
    /// Reset only zoom (scale to 1.0 or bounds to canvas)
    Zoom,
    /// Reset both position and zoom
    Both,
}

impl ResetMode {
    /// Convert to string representation for OBS driver
    pub fn as_str(&self) -> &'static str {
        match self {
            ResetMode::Position => "position",
            ResetMode::Zoom => "zoom",
            ResetMode::Both => "both",
        }
    }
}

/// Request body for resetting camera transform
#[derive(Debug, Deserialize)]
pub struct ResetCameraRequest {
    pub mode: ResetMode,
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
        .route("/api/cameras/:camera_id/reset", post(reset_camera_transform))
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
    let message = CameraStateMessage::TargetChanged {
        gamepad_slot: slot.clone(),
        camera_id: req.camera_id.clone(),
        timestamp: current_timestamp_millis(),
    };

    // Best-effort broadcast (ignore if no subscribers)
    let _ = state.update_tx.send(message);

    info!("Camera target set: {} -> {}", slot, req.camera_id);

    Ok(Json(serde_json::json!({
        "ok": true,
        "camera_id": req.camera_id
    })))
}

/// Get gamepad slots with their current camera targets populated
fn get_gamepads_with_targets(state: &ApiState) -> Vec<GamepadSlotInfo> {
    let mut slots = state.gamepad_slots.read().clone();
    for slot in &mut slots {
        slot.current_camera = state.camera_targets.get_target(&slot.slot);
    }
    slots
}

/// GET /api/gamepads - List all gamepad slots with their current camera targets
async fn list_gamepads(State(state): State<Arc<ApiState>>) -> Json<Vec<GamepadSlotInfo>> {
    Json(get_gamepads_with_targets(&state))
}

/// GET /api/cameras - List available cameras
async fn list_cameras(State(state): State<Arc<ApiState>>) -> Json<Vec<CameraInfo>> {
    Json(state.available_cameras.read().clone())
}

/// POST /api/cameras/:camera_id/reset - Reset camera transform (position and/or zoom)
async fn reset_camera_transform(
    Path(camera_id): Path<String>,
    State(state): State<Arc<ApiState>>,
    Json(req): Json<ResetCameraRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Validate camera_id exists and get camera info
    let camera = state
        .available_cameras
        .read()
        .iter()
        .find(|c| c.id == camera_id)
        .cloned();

    let camera = match camera {
        Some(c) => c,
        None => return Err(ApiError {
            error: format!(
                "Invalid camera_id: '{}'. Use GET /api/cameras to see available cameras.",
                camera_id
            ),
        }),
    };

    // Get OBS driver
    let obs_driver = match &state.obs_driver {
        Some(driver) => driver.clone(),
        None => return Err(ApiError {
            error: "OBS driver not available".to_string(),
        }),
    };

    // Reset transform (mode is already validated by serde deserialization)
    let mode_str = req.mode.as_str();
    if let Err(e) = obs_driver.reset_transform(&camera.scene, &camera.source, mode_str).await {
        error!("Failed to reset camera transform: {}", e);
        return Err(ApiError {
            error: format!("Failed to reset camera: {}", e),
        });
    }

    info!("Camera reset successful: camera={}, mode={}", camera_id, mode_str);

    Ok(Json(serde_json::json!({
        "ok": true,
        "camera_id": camera_id,
        "mode": mode_str
    })))
}

/// GET /api/ws/camera-updates - WebSocket for push notifications
async fn camera_updates_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApiState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

/// Handle WebSocket connection for camera updates
async fn handle_websocket(mut socket: WebSocket, state: Arc<ApiState>) {
    debug!("WebSocket client connected for camera updates");

    // Send initial snapshot on connect
    let snapshot = build_snapshot(&state);
    let snapshot_json = match serde_json::to_string(&snapshot) {
        Ok(json) => json,
        Err(e) => {
            error!("Failed to serialize snapshot: {}", e);
            return;
        }
    };

    if socket.send(Message::Text(snapshot_json.into())).await.is_err() {
        debug!("WebSocket client disconnected before receiving snapshot");
        return;
    }
    debug!("Sent initial snapshot to WebSocket client");

    // Subscribe to updates after sending snapshot
    let mut rx = state.update_tx.subscribe();

    loop {
        tokio::select! {
            // Forward updates to WebSocket
            result = rx.recv() => {
                match result {
                    Ok(message) => {
                        let msg = serde_json::to_string(&message).unwrap();
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

/// Build a snapshot of the current camera state
fn build_snapshot(state: &ApiState) -> CameraStateMessage {
    let gamepads = get_gamepads_with_targets(state);
    let cameras = state.available_cameras.read().clone();
    let on_air_camera = state.current_on_air_camera.read().clone();

    CameraStateMessage::Snapshot {
        gamepads,
        cameras,
        on_air_camera,
        timestamp: current_timestamp_millis(),
    }
}

/// Broadcast an ON AIR change event
///
/// Call this when the OBS program scene changes to notify WebSocket clients.
pub fn broadcast_on_air_change(state: &ApiState, camera_id: &str, scene_name: &str) {
    // Update the current on-air camera
    {
        let mut on_air = state.current_on_air_camera.write();
        *on_air = Some(camera_id.to_string());
    }

    // Broadcast the change
    let message = CameraStateMessage::OnAirChanged {
        camera_id: camera_id.to_string(),
        scene_name: scene_name.to_string(),
        timestamp: current_timestamp_millis(),
    };

    // Best-effort broadcast (ignore if no subscribers)
    let _ = state.update_tx.send(message);

    info!("ON AIR changed: {} (scene: {})", camera_id, scene_name);
}

/// Get current timestamp in milliseconds since UNIX epoch
fn current_timestamp_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
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
