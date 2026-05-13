//! Static action catalog for the WinAudio driver.
//!
//! Mirror of `obs/catalog.rs`: read-only metadata describing the actions an
//! editor user can pick from a dropdown when wiring a control. Runtime
//! dispatch is unaffected; the driver still parses `(action, params)` via
//! [`actions::parse_session_target`].

use crate::api_editor::action_catalog::{ActionDescriptor, ParamDescriptor, ParamKind};
use serde_json::json;

/// Build the static WinAudio action catalog.
pub fn winaudio_catalog() -> Vec<ActionDescriptor> {
    vec![
        ActionDescriptor::simple("master_volume", "Master volume").with_description(
            "Drive the default render endpoint volume from a fader (14-bit PitchBend).",
        ),
        ActionDescriptor::simple("master_mute", "Master mute")
            .with_description("Toggle the default render endpoint mute on button press."),
        ActionDescriptor::simple("session_volume", "Session volume")
            .with_description("Drive a per-app session volume from a fader.")
            .with_param(
                ParamDescriptor::new("target", ParamKind::String)
                    .with_picker("winaudio.target")
                    .with_default(json!("auto")),
            ),
        ActionDescriptor::simple("session_mute", "Session mute")
            .with_description("Toggle a per-app session mute on button press.")
            .with_param(
                ParamDescriptor::new("target", ParamKind::String)
                    .with_picker("winaudio.target")
                    .with_default(json!("auto")),
            ),
    ]
}
