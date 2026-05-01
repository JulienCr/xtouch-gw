//! YAML validation: parses to `AppConfig` and runs cross-field checks.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, response::Response, Json};
use serde::{Deserialize, Serialize};

use super::EditorState;
use crate::config::AppConfig;

#[derive(Deserialize)]
pub struct ValidateRequest {
    pub body: String,
}

#[derive(Serialize, Debug)]
pub struct ValidationIssue {
    pub field_path: String,
    pub level: &'static str,
    pub message: String,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ValidateResponse {
    Ok {
        ok: bool,
    },
    Errors {
        ok: bool,
        errors: Vec<ValidationIssue>,
    },
}

pub async fn validate(
    State(_s): State<Arc<EditorState>>,
    Json(req): Json<ValidateRequest>,
) -> Response {
    match serde_yaml::from_str::<AppConfig>(&req.body) {
        Ok(cfg) => {
            let issues = cross_field_checks(&cfg);
            if issues.is_empty() {
                Json(ValidateResponse::Ok { ok: true }).into_response()
            } else {
                Json(ValidateResponse::Errors {
                    ok: false,
                    errors: issues,
                })
                .into_response()
            }
        },
        Err(e) => {
            let location = e.location();
            let field_path = location
                .map(|l| format!("line {}, column {}", l.line(), l.column()))
                .unwrap_or_else(|| "(unknown)".to_string());
            let issue = ValidationIssue {
                field_path,
                level: "error",
                message: e.to_string(),
            };
            Json(ValidateResponse::Errors {
                ok: false,
                errors: vec![issue],
            })
            .into_response()
        },
    }
}

/// Cross-field validation: runs only after a successful parse.
///
/// Currently checks:
/// - `obs.camera_control.default_camera` references a known camera id
/// - No two pages share the same name (used for paging routing)
/// - Per-page `controls.*.indicator.signal` and `app` references are not validated
///   here (they require driver introspection).
pub fn cross_field_checks(cfg: &AppConfig) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    if let Some(obs) = &cfg.obs {
        if let Some(cc) = &obs.camera_control {
            let known: HashSet<&str> = cc.cameras.iter().map(|c| c.id.as_str()).collect();
            if let Some(default) = &cc.default_camera {
                if !known.contains(default.as_str()) {
                    issues.push(ValidationIssue {
                        field_path: "obs.camera_control.default_camera".into(),
                        level: "error",
                        message: format!(
                            "default_camera '{}' is not present in cameras list",
                            default
                        ),
                    });
                }
            }
            // duplicate camera ids
            let mut seen: HashSet<&str> = HashSet::new();
            for cam in &cc.cameras {
                if !seen.insert(cam.id.as_str()) {
                    issues.push(ValidationIssue {
                        field_path: format!("obs.camera_control.cameras[{}]", cam.id),
                        level: "error",
                        message: format!("duplicate camera id '{}'", cam.id),
                    });
                }
            }
        }
    }

    // duplicate page names
    let mut page_names: HashSet<&str> = HashSet::new();
    for (i, page) in cfg.pages.iter().enumerate() {
        if !page_names.insert(page.name.as_str()) {
            issues.push(ValidationIssue {
                field_path: format!("pages[{}].name", i),
                level: "error",
                message: format!("duplicate page name '{}'", page.name),
            });
        }
    }

    issues
}
