//! System tray UI module
//!
//! Provides a native Windows system tray interface for monitoring:
//! - Connection status for all drivers (OBS, QLC+, Voicemeeter)
//! - Real-time activity LEDs for in/out traffic
//! - Connect/recheck functionality for disconnected drivers

use std::sync::Arc;

// Module exports
pub mod activity;
pub mod handler;
pub mod icons;
pub mod manager;

// Re-exports
pub use activity::ActivityTracker;
pub use handler::TrayMessageHandler;
pub use manager::TrayManager;

/// Commands sent from tray UI to the main Tokio runtime
#[derive(Debug, Clone)]
pub enum TrayCommand {
    /// Attempt to connect/reconnect to OBS
    ConnectObs,
    /// Recheck all driver connections
    RecheckAll,
    /// Shutdown the application
    Shutdown,
}

/// Updates sent from the main runtime to the tray UI
#[derive(Debug, Clone)]
pub enum TrayUpdate {
    /// Driver connection status changed
    DriverStatus {
        name: String,
        status: ConnectionStatus,
    },
    /// Activity detected on a driver (deprecated - use ActivitySnapshot)
    Activity {
        driver: String,
        direction: ActivityDirection,
    },
    /// Periodic snapshot of all driver activity states (Phase 5)
    ActivitySnapshot {
        /// Map of (driver_name, direction) -> is_active
        activities: std::collections::HashMap<(String, ActivityDirection), bool>,
    },
}

/// Connection status for a driver
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    /// Driver is connected and operational
    Connected,
    /// Driver is disconnected
    Disconnected,
    /// Driver is attempting to reconnect
    Reconnecting { attempt: usize },
}

/// Direction of message activity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActivityDirection {
    /// Message received from application/hardware
    Inbound,
    /// Message sent to application/hardware
    Outbound,
}

/// Type alias for connection status callbacks
pub type StatusCallback = Arc<dyn Fn(ConnectionStatus) + Send + Sync>;
