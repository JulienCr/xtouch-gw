//! Live event bus for the editor's `/api/live` WebSocket.
//!
//! A small, dependency-free `tokio::sync::broadcast` channel that the router
//! and connection-aware drivers tap into. Subscribers (currently the editor
//! WebSocket handler) get a serializable feed of hardware events, driver
//! connection transitions, and config-reload notifications.
//!
//! The hot path emits with `tx.send(...).ok()` — never fail because nobody is
//! listening.

use serde::Serialize;

/// One unit of live activity broadcast to editor clients.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LiveEvent {
    /// A hardware control changed (X-Touch fader/button/encoder, gamepad axis/button).
    HwEvent {
        control_id: String,
        kind: HwEventKind,
        value: f32,
        ts: u64,
    },
    /// A driver / hardware target changed connection state.
    Connection {
        target: String,
        status: ConnectionStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        ts: u64,
    },
    /// The configuration file was successfully reloaded.
    ConfigReloaded { ts: u64 },
    /// The active page changed (X-Touch buttons, REPL, or editor request).
    PageChanged { index: usize, name: String, ts: u64 },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HwEventKind {
    Press,
    Release,
    Rotate,
    Axis,
    Fader,
    Encoder,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Up,
    Down,
}

/// Cloneable broadcast sender used across the binary.
pub type LiveEventTx = tokio::sync::broadcast::Sender<LiveEvent>;

/// Construct a new live event channel with the given buffer capacity.
///
/// Dropped messages (slow subscriber) surface as `RecvError::Lagged` on the
/// receiver side; the WS handler logs and continues.
pub fn channel(capacity: usize) -> (LiveEventTx, tokio::sync::broadcast::Receiver<LiveEvent>) {
    tokio::sync::broadcast::channel(capacity)
}

/// Default buffer capacity (events).
pub const DEFAULT_CAPACITY: usize = 256;

/// Current millisecond timestamp (UNIX epoch). Convenience for emitters.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
