//! Shared button mapping for gilrs controllers
//!
//! This module provides a canonical mapping from gilrs button positions to
//! Xbox-style button names. Third-party controllers (like FaceOff) typically
//! report using Nintendo physical layout, where button positions differ from
//! Xbox conventions.
//!
//! # Physical Layout Mapping
//!
//! The gilrs library reports buttons by physical position (South, East, North, West).
//! This module maps those positions to Xbox-style names assuming Nintendo layout:
//!
//! ```text
//!       [X/North]           (top)
//!   [Y/West] [A/East]       (left/right)
//!       [B/South]           (bottom)
//! ```
//!
//! This matches what most third-party controllers (FaceOff, 8BitDo, etc.) report.

use gilrs::Button;
use tracing::warn;

/// Map gilrs button position to Xbox-style button name
///
/// Uses Nintendo physical layout (what most third-party controllers report):
/// - East (right) -> "a"
/// - South (bottom) -> "b"
/// - North (top) -> "x"
/// - West (left) -> "y"
///
/// Returns `None` for D-Pad buttons (use `gilrs_dpad_to_name` instead) and
/// unknown buttons.
pub fn gilrs_button_to_name(button: Button) -> Option<&'static str> {
    match button {
        // Face buttons (Nintendo layout -> Xbox names)
        Button::East => Some("a"),  // Right = A
        Button::South => Some("b"), // Bottom = B
        Button::North => Some("x"), // Top = X
        Button::West => Some("y"),  // Left = Y

        // Shoulder buttons
        Button::LeftTrigger => Some("lb"),
        Button::RightTrigger => Some("rb"),
        Button::LeftTrigger2 => Some("lt"),
        Button::RightTrigger2 => Some("rt"),

        // Menu buttons
        Button::Select => Some("minus"),
        Button::Start => Some("plus"),
        Button::Mode => Some("home"),

        // Stick clicks
        Button::LeftThumb => Some("l3"),
        Button::RightThumb => Some("r3"),

        // D-Pad handled separately
        Button::DPadUp | Button::DPadDown | Button::DPadLeft | Button::DPadRight => None,

        // Other buttons
        Button::C => Some("c"),
        Button::Z => Some("capture"),

        // Unknown
        _ => {
            warn!("Unknown gilrs button: {:?}", button);
            None
        },
    }
}

/// Map gilrs D-Pad button to direction name (without prefix)
///
/// Returns the direction name ("up", "down", "left", "right") for D-Pad buttons,
/// or `None` for non-D-Pad buttons.
pub fn gilrs_dpad_to_name(button: Button) -> Option<&'static str> {
    match button {
        Button::DPadUp => Some("up"),
        Button::DPadDown => Some("down"),
        Button::DPadLeft => Some("left"),
        Button::DPadRight => Some("right"),
        _ => None,
    }
}

/// Convert gilrs button to full control ID with prefix
///
/// Combines `gilrs_button_to_name` and `gilrs_dpad_to_name` to produce
/// a complete control ID like "gamepad1.btn.a" or "gamepad1.dpad.up".
///
/// Returns `None` for unknown buttons.
pub fn gilrs_button_to_control_id(button: Button, prefix: &str) -> Option<String> {
    if let Some(dir) = gilrs_dpad_to_name(button) {
        Some(format!("{}.dpad.{}", prefix, dir))
    } else {
        gilrs_button_to_name(button).map(|name| format!("{}.btn.{}", prefix, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_face_button_mapping_nintendo_layout() {
        // Nintendo layout: A=East, B=South, X=North, Y=West
        assert_eq!(gilrs_button_to_name(Button::East), Some("a"));
        assert_eq!(gilrs_button_to_name(Button::South), Some("b"));
        assert_eq!(gilrs_button_to_name(Button::North), Some("x"));
        assert_eq!(gilrs_button_to_name(Button::West), Some("y"));
    }

    #[test]
    fn test_shoulder_buttons() {
        assert_eq!(gilrs_button_to_name(Button::LeftTrigger), Some("lb"));
        assert_eq!(gilrs_button_to_name(Button::RightTrigger), Some("rb"));
        assert_eq!(gilrs_button_to_name(Button::LeftTrigger2), Some("lt"));
        assert_eq!(gilrs_button_to_name(Button::RightTrigger2), Some("rt"));
    }

    #[test]
    fn test_dpad_buttons() {
        // D-Pad returns None from gilrs_button_to_name
        assert_eq!(gilrs_button_to_name(Button::DPadUp), None);
        assert_eq!(gilrs_button_to_name(Button::DPadDown), None);

        // Use gilrs_dpad_to_name instead
        assert_eq!(gilrs_dpad_to_name(Button::DPadUp), Some("up"));
        assert_eq!(gilrs_dpad_to_name(Button::DPadDown), Some("down"));
        assert_eq!(gilrs_dpad_to_name(Button::DPadLeft), Some("left"));
        assert_eq!(gilrs_dpad_to_name(Button::DPadRight), Some("right"));
    }

    #[test]
    fn test_control_id_generation() {
        assert_eq!(
            gilrs_button_to_control_id(Button::East, "gamepad1"),
            Some("gamepad1.btn.a".to_string())
        );
        assert_eq!(
            gilrs_button_to_control_id(Button::DPadUp, "gamepad1"),
            Some("gamepad1.dpad.up".to_string())
        );
    }
}
