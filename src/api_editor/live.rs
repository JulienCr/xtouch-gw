//! `/api/live` WebSocket handler.
//!
//! Streams `LiveEvent` JSON messages produced by the router and connection
//! taps. Best-effort: if a subscriber lags behind the broadcast buffer the
//! handler logs and continues, dropping intermediate events.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::event_bus::{ConnectionStatus, HwEventKind, LiveEvent};

use super::EditorState;

/// `GET /api/live` upgrade endpoint.
pub async fn ws(
    upgrade: WebSocketUpgrade,
    State(state): State<Arc<EditorState>>,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<EditorState>) {
    debug!("editor /api/live: client connected");

    // Subscribe (or send an empty hello if no bus is wired).
    let mut rx = match state.live_tx.as_ref() {
        Some(tx) => tx.subscribe(),
        None => {
            let hello = LiveEvent::Connection {
                target: "live".into(),
                status: ConnectionStatus::Down,
                detail: Some("live event bus not wired".into()),
                ts: crate::event_bus::now_ms(),
            };
            if let Ok(json) = serde_json::to_string(&hello) {
                let _ = socket.send(Message::Text(json)).await;
            }
            return;
        },
    };

    // Optional initial hello so the UI gets at least one frame.
    let hello = LiveEvent::Connection {
        target: "live".into(),
        status: ConnectionStatus::Up,
        detail: None,
        ts: crate::event_bus::now_ms(),
    };
    if let Ok(json) = serde_json::to_string(&hello) {
        if socket.send(Message::Text(json)).await.is_err() {
            return;
        }
    }

    // Push initial active page so the editor's PageTabs mirrors the router.
    if let Some(reader) = state.active_page_reader.as_ref() {
        if let Some((index, name)) = reader().await {
            let snap = LiveEvent::PageChanged {
                index,
                name,
                ts: crate::event_bus::now_ms(),
            };
            if let Ok(json) = serde_json::to_string(&snap) {
                if socket.send(Message::Text(json)).await.is_err() {
                    return;
                }
            }
        }
    }

    // Push initial fader snapshot so the editor's virtual surface mirrors
    // the current motorized fader positions without waiting for movement.
    if let Some(reader) = state.fader_setpoint.as_ref() {
        for ch in 1u8..=9 {
            let Some(v14) = reader(ch) else { continue };
            let control_id = if ch == 9 {
                "fader_master".to_string()
            } else {
                format!("fader{ch}")
            };
            let snap = LiveEvent::HwEvent {
                control_id,
                kind: HwEventKind::Fader,
                value: (v14 as f32) / 16383.0,
                ts: crate::event_bus::now_ms(),
            };
            if let Ok(json) = serde_json::to_string(&snap) {
                if socket.send(Message::Text(json)).await.is_err() {
                    return;
                }
            }
        }
    }

    loop {
        tokio::select! {
            recv = rx.recv() => match recv {
                Ok(event) => {
                    let json = match serde_json::to_string(&event) {
                        Ok(j) => j,
                        Err(e) => {
                            warn!("/api/live: failed to serialize event: {}", e);
                            continue;
                        },
                    };
                    if socket.send(Message::Text(json)).await.is_err() {
                        debug!("/api/live: client disconnected during send");
                        break;
                    }
                },
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("/api/live: bus closed");
                    break;
                },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("/api/live: subscriber lagged by {} messages", n);
                },
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None => {
                    debug!("/api/live: client closed connection");
                    break;
                },
                Some(Ok(Message::Ping(data))) => {
                    if socket.send(Message::Pong(data)).await.is_err() {
                        break;
                    }
                },
                Some(Ok(_)) => { /* ignore */ },
                Some(Err(e)) => {
                    warn!("/api/live: socket error: {}", e);
                    break;
                },
            },
        }
    }
}
