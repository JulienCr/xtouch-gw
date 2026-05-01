//! Integration tests for the editor API routes.
//!
//! Builds the editor router directly (without the rest of `ApiState`) on top
//! of a tempdir-backed `ProfileStore` and exercises it via `tower::oneshot`.

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use tempfile::TempDir;
use tower::ServiceExt;

use xtouch_gw::api_editor::{routes, ActionDescriptor, EditorState, ParamDescriptor, ParamKind};
use xtouch_gw::config::profiles::ProfileStore;

const SAMPLE_YAML: &str = r#"midi:
  input_port: in
  output_port: out
pages: []
"#;

struct Fx {
    _tmp: TempDir,
    router: Router,
    store: Arc<ProfileStore>,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    let watched = tmp.path().join("config.yaml");
    std::fs::write(&watched, SAMPLE_YAML).unwrap();
    let store = Arc::new(ProfileStore::new(root, watched, 50));
    store.ensure_initialized().unwrap();
    let state = Arc::new(EditorState::with_profiles(Arc::clone(&store)));
    let router = routes().with_state(state);
    Fx {
        _tmp: tmp,
        router,
        store,
    }
}

async fn send(router: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body_bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let value: serde_json::Value = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, value)
}

fn json_req(method: Method, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn lists_default_profile_after_init() {
    let f = fx();
    let (status, body) = send(&f.router, get("/api/profiles")).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "default");
    assert_eq!(arr[0]["is_active"], true);
}

#[tokio::test]
async fn create_duplicate_rename_delete_flow() {
    let f = fx();

    // Create blank
    let (s, _) = send(
        &f.router,
        json_req(
            Method::POST,
            "/api/profiles",
            serde_json::json!({ "name": "scratch" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // Duplicate
    let (s, _) = send(
        &f.router,
        json_req(
            Method::POST,
            "/api/profiles/scratch/duplicate",
            serde_json::json!({ "new_name": "scratch_copy" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // Rename
    let (s, _) = send(
        &f.router,
        json_req(
            Method::POST,
            "/api/profiles/scratch_copy/rename",
            serde_json::json!({ "new_name": "renamed" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Delete (non-active)
    let (s, _) = send(
        &f.router,
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/profiles/renamed")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Active profile cannot be deleted
    let (s, _) = send(
        &f.router,
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/profiles/default")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn save_with_stale_hash_returns_conflict_with_current() {
    let f = fx();

    let (s, _) = send(
        &f.router,
        json_req(
            Method::POST,
            "/api/profiles",
            serde_json::json!({ "name": "scratch", "body": "v: 1\n" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    let (s, body) = send(
        &f.router,
        json_req(
            Method::PUT,
            "/api/profiles/scratch",
            serde_json::json!({ "body": "v: 2\n", "expected_hash": "deadbeef" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(body["current_hash"].is_string());
    assert!(body["current_body"].is_string());

    // sanity: store unchanged
    let (saved, _) = f.store.read("scratch").unwrap();
    assert_eq!(saved, "v: 1\n");
}

#[tokio::test]
async fn validate_rejects_bad_yaml() {
    let f = fx();
    let (s, body) = send(
        &f.router,
        json_req(
            Method::POST,
            "/api/validate",
            serde_json::json!({ "body": "this is: not: valid: yaml: at: all" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert!(body["errors"].as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn validate_accepts_minimal_config() {
    let f = fx();
    let (s, body) = send(
        &f.router,
        json_req(
            Method::POST,
            "/api/validate",
            serde_json::json!({ "body": SAMPLE_YAML }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["ok"], true);
}

/// Build a fixture with a populated driver catalog (no live OBS).
fn fx_with_catalog() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    let watched = tmp.path().join("config.yaml");
    std::fs::write(&watched, SAMPLE_YAML).unwrap();
    let store = Arc::new(ProfileStore::new(root, watched, 50));
    store.ensure_initialized().unwrap();

    let mut catalogs = std::collections::HashMap::new();
    catalogs.insert(
        "obs".to_string(),
        vec![
            ActionDescriptor::simple("changeScene", "Change scene").with_param(
                ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"),
            ),
            ActionDescriptor::simple("nudgeX", "Nudge X"),
        ],
    );

    let state = Arc::new(EditorState {
        profiles: Arc::clone(&store),
        live_tx: None,
        obs: None,
        drivers: Arc::new(catalogs),
    });
    let router = routes().with_state(state);
    Fx {
        _tmp: tmp,
        router,
        store,
    }
}

#[tokio::test]
async fn drivers_actions_returns_catalog() {
    let f = fx_with_catalog();
    let (status, body) = send(&f.router, get("/api/drivers/obs/actions")).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().expect("array");
    assert!(!arr.is_empty(), "expected non-empty obs catalog");
    let names: Vec<&str> = arr.iter().filter_map(|v| v["name"].as_str()).collect();
    assert!(names.contains(&"changeScene"));
}

#[tokio::test]
async fn drivers_actions_unknown_driver_returns_404() {
    let f = fx_with_catalog();
    let (status, _) = send(&f.router, get("/api/drivers/nope/actions")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn drivers_lists_registered_names() {
    let f = fx_with_catalog();
    let (status, body) = send(&f.router, get("/api/drivers")).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().expect("array");
    assert!(arr.iter().any(|v| v["name"] == "obs"));
}

#[tokio::test]
async fn obs_scenes_returns_503_when_unwired() {
    let f = fx(); // no obs picker source wired
    let (status, body) = send(&f.router, get("/api/obs/scenes")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "obs_not_connected");
}

#[tokio::test]
async fn obs_inputs_returns_503_when_unwired() {
    let f = fx();
    let (status, body) = send(&f.router, get("/api/obs/inputs")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "obs_not_connected");
}

#[tokio::test]
async fn live_ws_route_is_registered() {
    // We don't perform a full WS handshake here (oneshot doesn't drive the
    // upgrade machinery); we just ensure the route exists and is wired to a
    // WebSocketUpgrade extractor. A non-WS GET should yield 4xx (Upgrade
    // Required / Bad Request), NOT 404.
    let f = fx();
    let resp = f.router.clone().oneshot(get("/api/live")).await.unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "/api/live should be registered"
    );
    assert!(
        resp.status().is_client_error() || resp.status().is_informational(),
        "expected 4xx/1xx for non-upgrade GET, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn schema_endpoint_returns_object() {
    let f = fx();
    let (s, body) = send(&f.router, get("/api/schema")).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.is_object());
    let has_known_key = body.get("$schema").is_some()
        || body.get("definitions").is_some()
        || body.get("$defs").is_some()
        || body.get("properties").is_some();
    assert!(
        has_known_key,
        "schema response missing expected key: {}",
        body
    );
}
