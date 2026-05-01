//! Static action catalog for the OBS driver.
//!
//! Read-only metadata describing the actions an editor user can pick from a
//! dropdown when wiring a control. The catalog mirrors the dispatch table in
//! `actions.rs` but is intentionally not exhaustive — niche actions can be
//! typed manually in YAML if absent.

use crate::api_editor::action_catalog::{ActionDescriptor, ParamDescriptor, ParamKind};
use serde_json::json;

/// Build the static OBS action catalog.
pub fn obs_catalog() -> Vec<ActionDescriptor> {
    vec![
        ActionDescriptor::simple("changeScene", "Change scene")
            .with_description("Switch program (or preview when in studio mode) to the given scene.")
            .with_param(
                ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"),
            ),
        ActionDescriptor::simple("setScene", "Set scene (alias)")
            .with_description("Identical to changeScene; kept for legacy YAML.")
            .with_param(
                ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"),
            ),
        ActionDescriptor::simple("toggleStudioMode", "Toggle studio mode"),
        ActionDescriptor::simple("TriggerStudioModeTransition", "Trigger studio transition"),
        ActionDescriptor::simple("nudgeX", "Nudge X (pan)")
            .with_description("Move the camera source horizontally by `step` pixels.")
            .with_param(ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"))
            .with_param(
                ParamDescriptor::new("source", ParamKind::SourceRef).with_picker("obs.source"),
            )
            .with_param(ParamDescriptor::new("step", ParamKind::Number).with_default(json!(2.0))),
        ActionDescriptor::simple("nudgeY", "Nudge Y (tilt)")
            .with_description("Move the camera source vertically by `step` pixels.")
            .with_param(ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"))
            .with_param(
                ParamDescriptor::new("source", ParamKind::SourceRef).with_picker("obs.source"),
            )
            .with_param(ParamDescriptor::new("step", ParamKind::Number).with_default(json!(2.0))),
        ActionDescriptor::simple("scaleUniform", "Scale uniformly (zoom)")
            .with_description("Apply a uniform scale delta to the camera source.")
            .with_param(ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"))
            .with_param(
                ParamDescriptor::new("source", ParamKind::SourceRef).with_picker("obs.source"),
            )
            .with_param(ParamDescriptor::new("step", ParamKind::Number).with_default(json!(0.02))),
        ActionDescriptor::simple("resetPosition", "Reset position")
            .with_param(ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"))
            .with_param(
                ParamDescriptor::new("source", ParamKind::SourceRef).with_picker("obs.source"),
            ),
        ActionDescriptor::simple("resetZoom", "Reset zoom")
            .with_param(ParamDescriptor::new("scene", ParamKind::SceneRef).with_picker("obs.scene"))
            .with_param(
                ParamDescriptor::new("source", ParamKind::SourceRef).with_picker("obs.source"),
            ),
        ActionDescriptor::simple("selectCamera", "Select camera")
            .with_description("Switch the active camera target (program or preview).")
            .with_param(ParamDescriptor::new("camera_id", ParamKind::String))
            .with_param(
                ParamDescriptor::new("target", ParamKind::String).with_default(json!("preview")),
            ),
        ActionDescriptor::simple("enterSplit", "Enter split view"),
        ActionDescriptor::simple("toggleSplit", "Toggle split view"),
        ActionDescriptor::simple("exitSplit", "Exit split view"),
        ActionDescriptor::simple("setPtzModifier", "PTZ modifier (button hold)"),
    ]
}
