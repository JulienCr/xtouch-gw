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
mod connection;
mod driver;
mod encoder;
mod transform;

// Re-export main types
pub use driver::ObsDriver;

// Re-export Driver trait and types from parent
use super::{Driver, ExecutionContext, IndicatorCallback};
