//! MIDI port enumeration for the editor's port-picker UI.

use axum::{
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use midir::{MidiIO, MidiInput, MidiOutput};
use serde::Serialize;
use tracing::warn;

#[derive(Serialize)]
pub struct PortsResponse {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

/// `GET /api/midi/ports` — best-effort enumeration of MIDI input/output ports.
///
/// On failure to construct a midir client (rare, e.g. backend init issue), we
/// return empty arrays plus an `X-Midi-Warning` header so the UI can surface a
/// non-fatal hint.
pub async fn ports() -> Response {
    let mut warning: Option<String> = None;

    let inputs = match MidiInput::new("XTouch-GW-Editor-PortList") {
        Ok(midi) => collect_port_names(&midi),
        Err(e) => {
            warn!("MIDI input enumeration failed: {}", e);
            warning = Some(format!("input: {}", e));
            Vec::new()
        },
    };

    let outputs = match MidiOutput::new("XTouch-GW-Editor-PortList") {
        Ok(midi) => collect_port_names(&midi),
        Err(e) => {
            warn!("MIDI output enumeration failed: {}", e);
            let prev = warning
                .take()
                .map(|w| format!("{}; ", w))
                .unwrap_or_default();
            warning = Some(format!("{}output: {}", prev, e));
            Vec::new()
        },
    };

    let mut headers = HeaderMap::new();
    if let Some(w) = warning {
        if let Ok(val) = HeaderValue::from_str(&w) {
            headers.insert("X-Midi-Warning", val);
        }
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    (headers, Json(PortsResponse { inputs, outputs })).into_response()
}

fn collect_port_names<T: MidiIO>(midi: &T) -> Vec<String> {
    midi.ports()
        .into_iter()
        .filter_map(|p| midi.port_name(&p).ok())
        .collect()
}
