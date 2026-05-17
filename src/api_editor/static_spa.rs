//! Static SPA serving for the editor under `/editor/*`.
//!
//! In **release** builds, assets from `editor/build/` are embedded into the
//! binary via `rust-embed` (with compression).
//!
//! In **debug** builds, we redirect to the Vite dev server at
//! `http://localhost:5173`. Run `pnpm --dir editor dev` separately.

use std::sync::Arc;

use axum::http::StatusCode;
#[cfg(not(debug_assertions))]
use axum::http::{header, HeaderMap, HeaderValue};
#[cfg(debug_assertions)]
use axum::response::Redirect;
use axum::{
    extract::Path,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use super::EditorState;

#[cfg(not(debug_assertions))]
#[derive(rust_embed::RustEmbed)]
#[folder = "editor/build/"]
struct Asset;

pub fn routes() -> Router<Arc<EditorState>> {
    Router::new()
        .route("/editor", get(index))
        .route("/editor/", get(index))
        .route("/editor/*path", get(asset_or_index))
}

async fn index() -> Response {
    serve_index()
}

async fn asset_or_index(Path(path): Path<String>) -> Response {
    serve_path(&path)
}

// ---------- release: serve from embed ----------

#[cfg(not(debug_assertions))]
fn serve_index() -> Response {
    serve_embedded("index.html", /* immutable= */ false)
        .unwrap_or_else(|| (StatusCode::NOT_FOUND, "index.html missing").into_response())
}

#[cfg(not(debug_assertions))]
fn serve_path(path: &str) -> Response {
    // Try the exact asset first.
    if let Some(resp) = serve_embedded(path, is_immutable(path)) {
        return resp;
    }
    // SPA fallback: deep links like /editor/profiles/foo serve index.html
    // so the client-side router can take over.
    serve_embedded("index.html", false)
        .unwrap_or_else(|| (StatusCode::NOT_FOUND, "not found").into_response())
}

#[cfg(not(debug_assertions))]
fn serve_embedded(path: &str, immutable: bool) -> Option<Response> {
    let asset = Asset::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let mut headers = HeaderMap::new();
    if let Ok(ct) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(header::CONTENT_TYPE, ct);
    }
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));

    Some((headers, asset.data.into_owned()).into_response())
}

#[cfg(not(debug_assertions))]
fn is_immutable(path: &str) -> bool {
    path.starts_with("_app/immutable/")
}

// ---------- debug: redirect to vite dev server ----------

#[cfg(debug_assertions)]
fn serve_index() -> Response {
    Redirect::temporary("http://localhost:5173/editor").into_response()
}

/// Audit #72: axum URL-decodes path segments, so `..%2F..%2Fadmin` arrives
/// here as `../../admin` and would be substituted verbatim into the Location
/// header, escaping the `/editor/` prefix. Reject any decoded path that
/// could break out of the editor namespace.
fn path_is_safe(path: &str) -> bool {
    !path.contains("..") && !path.starts_with('/') && !path.contains('\\')
}

#[cfg(debug_assertions)]
fn serve_path(path: &str) -> Response {
    if !path_is_safe(path) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let target = format!("http://localhost:5173/editor/{}", path);
    Redirect::temporary(&target).into_response()
}

#[cfg(test)]
mod tests {
    use super::path_is_safe;

    #[test]
    fn rejects_parent_segment() {
        assert!(!path_is_safe("../admin"));
        assert!(!path_is_safe("foo/../bar"));
    }

    #[test]
    fn rejects_absolute_path() {
        assert!(!path_is_safe("/admin"));
    }

    #[test]
    fn rejects_backslash() {
        assert!(!path_is_safe("foo\\bar"));
    }

    #[test]
    fn accepts_simple_paths() {
        assert!(path_is_safe("index.html"));
        assert!(path_is_safe("_app/immutable/main.js"));
        assert!(path_is_safe("nested/file.css"));
    }
}
