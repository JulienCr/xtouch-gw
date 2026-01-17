//! Application drivers (OBS, Voicemeeter, etc.)
//!
//! Note: QLC+ is controlled via `MidiBridgeDriver` configured in `config.midi.apps`.
//! There is no separate QLC driver - the MIDI bridge handles all MIDI passthrough.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Callback type for indicator emission
///
/// Drivers call this to emit indicator signals (e.g., "obs.selectedScene", "voicemeeter.mute.1")
/// that control LED states on the X-Touch surface.
///
/// # Arguments
/// * `signal` - Signal name (e.g., "obs.studioMode", "obs.selectedScene")
/// * `value` - Signal value (boolean, string, number, etc.)
pub type IndicatorCallback = Arc<dyn Fn(String, Value) + Send + Sync>;

/// Execution context passed to drivers for accessing router state and config
#[derive(Clone)]
pub struct ExecutionContext {
    /// Application configuration
    pub config: Arc<RwLock<crate::config::AppConfig>>,
    /// Active page name
    pub active_page: Option<String>,
    /// Control value (for encoder/analog inputs)
    pub value: Option<serde_json::Value>,
    /// Control ID (e.g., "vpot1_rotate", "gamepad.left_stick_x")
    pub control_id: Option<String>,
    /// Activity tracker for tray UI (optional)
    pub activity_tracker: Option<Arc<crate::tray::ActivityTracker>>,
}

/// Driver trait - all application integrations implement this
///
/// Note: All methods take &self (not &mut self) to support Arc<dyn Driver>.
/// Drivers should use interior mutability (RwLock, Mutex, etc.) for mutable state.
#[async_trait]
pub trait Driver: Send + Sync {
    /// Get the driver name (e.g., "console", "obs", "voicemeeter")
    fn name(&self) -> &str;

    /// Initialize the driver (connect to application, open ports, etc.)
    /// Uses interior mutability - implement with RwLock/Mutex for state
    async fn init(&self, ctx: ExecutionContext) -> Result<()>;

    /// Execute an action with parameters
    ///
    /// # Arguments
    /// * `action` - The action name (e.g., "scene", "mute", "fader")
    /// * `params` - JSON parameters from config
    /// * `ctx` - Execution context for accessing router state
    async fn execute(&self, action: &str, params: Vec<Value>, ctx: ExecutionContext) -> Result<()>;

    /// Sync driver state (called after config reload)
    async fn sync(&self) -> Result<()>;

    /// Shutdown the driver gracefully
    async fn shutdown(&self) -> Result<()>;

    /// Subscribe to indicator signals from this driver
    ///
    /// The driver calls the provided callback with (signal_name, value) pairs
    /// whenever indicator state changes (e.g., scene changes, mute state).
    ///
    /// Default implementation: no-op (driver doesn't emit indicators)
    fn subscribe_indicators(&self, _callback: IndicatorCallback) {
        // Default: do nothing (not all drivers emit indicators)
    }

    /// Get current connection status
    ///
    /// Returns the current connection state of the driver.
    /// Default implementation: always connected (for drivers without network connections)
    fn connection_status(&self) -> crate::tray::ConnectionStatus {
        crate::tray::ConnectionStatus::Connected
    }

    /// Subscribe to connection status changes
    ///
    /// The driver calls the provided callback whenever connection status changes
    /// (e.g., connected, disconnected, reconnecting).
    ///
    /// Default implementation: no-op (driver doesn't track connection status)
    fn subscribe_connection_status(&self, _callback: crate::tray::StatusCallback) {
        // Default: do nothing (not all drivers track connection status)
    }
}

pub mod console;
pub mod midibridge;
pub mod obs;

// Re-export commonly used drivers
pub use console::ConsoleDriver;
pub use midibridge::MidiBridgeDriver;
pub use obs::ObsDriver;

// Suppress unused warnings temporarily during Phase 5 development
#[allow(unused_imports)]
use {ConsoleDriver as _, MidiBridgeDriver as _, ObsDriver as _};
