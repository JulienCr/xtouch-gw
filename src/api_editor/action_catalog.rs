//! Action catalog metadata exposed by the editor.
//!
//! The catalog is read-only: drivers describe the actions they support
//! (name, label, parameters, optional UI picker hint) so the editor can
//! render typed forms instead of free-text. It does NOT change runtime
//! dispatch — drivers still receive `(action, params)` and execute them.

use serde::Serialize;

/// One callable action a driver supports.
#[derive(Debug, Clone, Serialize)]
pub struct ActionDescriptor {
    /// Internal identifier dispatched on (e.g. "nudgeX").
    pub name: String,
    /// Human-readable label for the editor UI.
    pub label: String,
    /// Optional longer description / tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ordered parameter list.
    pub params: Vec<ParamDescriptor>,
}

/// One parameter slot of an action.
#[derive(Debug, Clone, Serialize)]
pub struct ParamDescriptor {
    pub name: String,
    pub kind: ParamKind,
    /// Editor picker hint, e.g. `obs.scene`, `obs.source`, `obs.input`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picker: Option<String>,
    /// Optional default value (any JSON shape).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// Coarse parameter type used by the editor to pick an input widget.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    String,
    Number,
    Integer,
    Boolean,
    SceneRef,
    SourceRef,
}

impl ActionDescriptor {
    /// Convenience builder for a no-params action.
    pub fn simple(name: &str, label: &str) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: None,
            params: Vec::new(),
        }
    }

    /// Builder: attach a description.
    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Builder: append a parameter.
    pub fn with_param(mut self, p: ParamDescriptor) -> Self {
        self.params.push(p);
        self
    }
}

impl ParamDescriptor {
    pub fn new(name: &str, kind: ParamKind) -> Self {
        Self {
            name: name.into(),
            kind,
            picker: None,
            default: None,
        }
    }
    pub fn with_picker(mut self, picker: &str) -> Self {
        self.picker = Some(picker.into());
        self
    }
    pub fn with_default(mut self, v: serde_json::Value) -> Self {
        self.default = Some(v);
        self
    }
}
