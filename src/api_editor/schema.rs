//! JSON Schema export for the application's `AppConfig` type.

use axum::{
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};

/// `GET /api/schema` — returns the JSON Schema for `AppConfig`.
///
/// Generated at request time (cheap, ~1ms). Cached for 60 seconds at the HTTP
/// layer to avoid hammering the generator on rapid reloads.
pub async fn schema() -> Response {
    let schema = schemars::schema_for!(crate::config::AppConfig);
    let value = serde_json::to_value(&schema).unwrap_or(serde_json::Value::Null);

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=60"),
    );
    (headers, Json(value)).into_response()
}
