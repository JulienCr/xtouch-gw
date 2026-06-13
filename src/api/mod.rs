//! REST API for Stream Deck integration
//!
//! Provides HTTP endpoints and WebSocket for controlling dynamic camera targets.
//! Default port: 8125

use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Request, State,
    },
    http::{header, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::api_editor as editor;

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
    /// Optional editor state. When `Some`, the editor data routes and the SPA
    /// are mounted by `build_router`.
    pub editor: Option<Arc<editor::EditorState>>,
    /// Port the API listens on. Threaded through so the CSRF Origin allowlist
    /// (audit #72) matches whatever port `start_server` actually binds.
    pub api_port: u16,
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
    /// Optional target: "preview" or "program". When set, also switches OBS scene.
    #[serde(default)]
    pub target: Option<String>,
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
    let api_port = state.api_port;

    let stream_deck = Router::new()
        .route(
            "/api/gamepad/:slot/camera",
            get(get_camera_target).put(set_camera_target),
        )
        .route("/api/gamepads", get(list_gamepads))
        .route("/api/cameras", get(list_cameras))
        .route(
            "/api/cameras/:camera_id/reset",
            post(reset_camera_transform),
        )
        .route("/api/ws/camera-updates", get(camera_updates_ws))
        .route("/api/health", get(health_check))
        .with_state(Arc::clone(&state));

    let mut router = stream_deck;
    if let Some(editor_state) = state.editor.clone() {
        router = router.merge(editor::routes().with_state(Arc::clone(&editor_state)));
        router = router.merge(editor::spa_routes().with_state(editor_state));
    }

    // Audit #72: CSRF Origin allowlist on mutating verbs. Loopback bind
    // already blocks LAN exposure, but a malicious page open in any browser
    // tab can issue cross-origin `fetch('http://127.0.0.1:8125/...', { mode:
    // 'no-cors' })` to DELETE/PUT/POST/PATCH our endpoints — browsers attach
    // `Origin: https://evil.com` on those even without preflight. We allow
    // requests with no `Origin` header (native HTTP clients like the Stream
    // Deck app and `curl` don't set one) and reject mismatching origins.
    router.layer(middleware::from_fn(move |req: Request, next: Next| {
        let port = api_port;
        async move { csrf_origin_guard(port, req, next).await }
    }))
}

fn is_mutating_method(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::DELETE | &Method::PATCH
    )
}

fn origin_is_loopback(origin: &str, port: u16) -> bool {
    // We don't allow https-loopback since the gateway only serves http.
    let expected_127 = format!("http://127.0.0.1:{}", port);
    let expected_localhost = format!("http://localhost:{}", port);
    origin == expected_127 || origin == expected_localhost
}

async fn csrf_origin_guard(api_port: u16, req: Request, next: Next) -> Response {
    if !is_mutating_method(req.method()) {
        return next.run(req).await;
    }

    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    match origin.as_deref() {
        None => {
            // Native HTTP clients (curl, Stream Deck) do not set Origin.
            // Cross-origin browser requests do — even `mode: 'no-cors'`
            // ones — so absence here is a strong signal it's safe.
            debug!(%method, %path, "csrf: allowing request with no Origin header");
            next.run(req).await
        },
        Some(origin) if origin_is_loopback(origin, api_port) => next.run(req).await,
        Some(origin) => {
            warn!(
                %method, %path, %origin,
                "csrf: rejecting cross-origin mutating request"
            );
            (
                StatusCode::FORBIDDEN,
                Json(ApiError {
                    error: "csrf: origin not allowed".into(),
                }),
            )
                .into_response()
        },
    }
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
///
/// When `target` is provided ("preview" or "program"), also switches OBS scene.
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

    // Set the PTZ target
    if let Err(e) = state.camera_targets.set_target(&slot, &req.camera_id) {
        error!("Failed to set camera target: {}", e);
        return Err(ApiError {
            error: format!("Failed to set camera target: {}", e),
        });
    }

    // If target mode is specified, also switch OBS scene
    if let Some(ref target) = req.target {
        if target == "preview" || target == "program" {
            if let Some(ref obs_driver) = state.obs_driver {
                if let Err(e) = obs_driver.select_camera(&req.camera_id, target).await {
                    warn!("Failed to switch OBS scene: {}", e);
                    // Don't fail the request - PTZ target was set successfully
                }
            }
        }
    }

    // Broadcast update to WebSocket subscribers
    let message = CameraStateMessage::TargetChanged {
        gamepad_slot: slot.clone(),
        camera_id: req.camera_id.clone(),
        timestamp: current_timestamp_millis(),
    };

    // Best-effort broadcast (ignore if no subscribers)
    let _ = state.update_tx.send(message);

    info!(
        "Camera target set: {} -> {}{}",
        slot,
        req.camera_id,
        req.target
            .as_ref()
            .map(|t| format!(" ({})", t))
            .unwrap_or_default()
    );

    Ok(Json(serde_json::json!({
        "ok": true,
        "camera_id": req.camera_id,
        "target": req.target
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
        None => {
            return Err(ApiError {
                error: format!(
                    "Invalid camera_id: '{}'. Use GET /api/cameras to see available cameras.",
                    camera_id
                ),
            })
        },
    };

    // Get OBS driver
    let obs_driver = match &state.obs_driver {
        Some(driver) => driver.clone(),
        None => {
            return Err(ApiError {
                error: "OBS driver not available".to_string(),
            })
        },
    };

    // Reset transform (mode is already validated by serde deserialization)
    let mode_str = req.mode.as_str();
    if let Err(e) = obs_driver
        .reset_transform(&camera.scene, &camera.source, mode_str)
        .await
    {
        error!("Failed to reset camera transform: {}", e);
        return Err(ApiError {
            error: format!("Failed to reset camera: {}", e),
        });
    }

    info!(
        "Camera reset successful: camera={}, mode={}",
        camera_id, mode_str
    );

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
    headers: axum::http::HeaderMap,
) -> Response {
    // The CSRF middleware (audit #72) only guards mutating HTTP verbs, but a WS
    // upgrade is a GET — and browsers allow cross-origin WebSocket connects
    // (sending an `Origin` header). Without this check a malicious page could
    // open the loopback socket and read the snapshot + live updates. Apply the
    // same Origin policy: allow no-Origin (native clients) and loopback, reject
    // anything else.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if !origin_is_loopback(origin, state.api_port) {
            warn!(%origin, "csrf: rejecting cross-origin WebSocket upgrade");
            return (
                StatusCode::FORBIDDEN,
                Json(ApiError {
                    error: "csrf: origin not allowed".into(),
                }),
            )
                .into_response();
        }
    }
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
        },
    };

    if socket.send(Message::Text(snapshot_json)).await.is_err() {
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
                        if socket.send(Message::Text(msg)).await.is_err() {
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

/// Broadcast camera target change to all connected Stream Deck clients
///
/// Call this when the gamepad camera target changes (e.g., from OBS preview scene change).
pub fn broadcast_target_change(state: &ApiState, gamepad_slot: &str, camera_id: &str) {
    let message = CameraStateMessage::TargetChanged {
        gamepad_slot: gamepad_slot.to_string(),
        camera_id: camera_id.to_string(),
        timestamp: current_timestamp_millis(),
    };

    // Best-effort broadcast (ignore if no subscribers)
    let _ = state.update_tx.send(message);

    info!("Target changed: {} -> {}", gamepad_slot, camera_id);
}

/// Get current timestamp in milliseconds since UNIX epoch
pub fn current_timestamp_millis() -> u64 {
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

    // Loopback-only by default. The Stream Deck app, the editor SPA, and any
    // local tooling all connect via 127.0.0.1; binding to all interfaces would
    // expose the API (including profile mutation endpoints) to the LAN.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    info!("Starting API server on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind API server")?;

    axum::serve(listener, router)
        .await
        .context("API server error")?;

    Ok(())
}

#[cfg(test)]
mod csrf_tests {
    use super::*;

    #[test]
    fn loopback_127_and_localhost_match() {
        assert!(origin_is_loopback("http://127.0.0.1:8125", 8125));
        assert!(origin_is_loopback("http://localhost:8125", 8125));
    }

    #[test]
    fn https_loopback_rejected() {
        // We only serve plain HTTP; an https Origin must not be treated as
        // same-origin or someone proxying our endpoints over TLS could spoof
        // the allowlist trivially.
        assert!(!origin_is_loopback("https://127.0.0.1:8125", 8125));
    }

    #[test]
    fn wrong_port_rejected() {
        assert!(!origin_is_loopback("http://127.0.0.1:9000", 8125));
    }

    #[test]
    fn foreign_origin_rejected() {
        assert!(!origin_is_loopback("https://evil.com", 8125));
    }

    #[test]
    fn mutating_methods_covered() {
        assert!(is_mutating_method(&Method::POST));
        assert!(is_mutating_method(&Method::PUT));
        assert!(is_mutating_method(&Method::DELETE));
        assert!(is_mutating_method(&Method::PATCH));
    }

    #[test]
    fn safe_methods_not_treated_as_mutating() {
        assert!(!is_mutating_method(&Method::GET));
        assert!(!is_mutating_method(&Method::HEAD));
        assert!(!is_mutating_method(&Method::OPTIONS));
    }
}
