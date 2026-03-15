//! OBS Studio WebSocket Driver
//!
//! Provides integration with OBS Studio via WebSocket protocol for:
//! - Scene switching (program/preview based on studio mode)
//! - Item transformation (position, scale)
//! - Studio mode control
//! - Automatic reconnection

// Module declarations
mod actions;
mod analog;
mod camera;
mod camera_actions;
mod connection;
mod driver;
mod encoder;
mod event_listener;
mod ptz_actions;
mod split_mode;
mod transform;

// Re-export main types
pub use driver::ObsDriver;

// Re-export Driver trait and types from parent
use super::{Driver, ExecutionContext, IndicatorCallback};

/// OBS indicator signal names (shared between emitter and consumers).
pub mod signals {
    pub const CURRENT_PROGRAM_SCENE: &str = "obs.currentProgramScene";
    pub const CURRENT_PREVIEW_SCENE: &str = "obs.currentPreviewScene";
    pub const STUDIO_MODE: &str = "obs.studioMode";
    pub const SELECTED_SCENE: &str = "obs.selectedScene";
}
