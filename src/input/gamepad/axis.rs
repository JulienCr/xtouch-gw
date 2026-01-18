//! Shared axis utilities for gamepad providers
//!
//! This module consolidates common axis-related functionality used by both
//! `provider.rs` (gilrs-only) and `hybrid_provider.rs` (XInput + gilrs).

use gilrs::Axis;
use tracing::warn;

/// Map gilrs axis to standardized control ID
///
/// Converts a gilrs Axis enum to the control ID format used in configuration:
/// - `LeftStickX` -> `{prefix}.axis.lx`
/// - `LeftStickY` -> `{prefix}.axis.ly`
/// - etc.
///
/// # Arguments
/// * `axis` - The gilrs axis enum
/// * `prefix` - The gamepad prefix (e.g., "gamepad1", "gamepad2")
///
/// # Returns
/// A control ID string in the format `{prefix}.axis.{name}`
pub fn gilrs_axis_to_control_id(axis: Axis, prefix: &str) -> String {
    let name = match axis {
        Axis::LeftStickX => "lx",
        Axis::LeftStickY => "ly",
        Axis::RightStickX => "rx",
        Axis::RightStickY => "ry",
        Axis::LeftZ => "zl",
        Axis::RightZ => "zr",
        _ => {
            warn!("Unknown gilrs axis: {:?}", axis);
            "unknown"
        },
    };

    format!("{}.axis.{}", prefix, name)
}
