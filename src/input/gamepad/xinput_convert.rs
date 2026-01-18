//! XInput state to GamepadEvent conversion
//!
//! Converts rusty_xinput controller state to standardized GamepadEvent format,
//! ensuring consistency with gilrs-based events.

use super::normalize::normalize_stick_radial;
use super::provider::GamepadEvent;
use crate::config::AnalogConfig;
use rusty_xinput::{XInputHandle, XInputState, XInputUsageError};

/// XInput button bit flags
///
/// rusty_xinput doesn't export individual button constants,
/// so we define them here based on the XInput API spec.
mod button_flags {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    pub const DPAD_LEFT: u16 = 0x0004;
    pub const DPAD_RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LEFT_THUMB: u16 = 0x0040;
    pub const RIGHT_THUMB: u16 = 0x0080;
    pub const LEFT_SHOULDER: u16 = 0x0100;
    pub const RIGHT_SHOULDER: u16 = 0x0200;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// Cached XInput controller state for change detection
#[derive(Debug, Clone)]
pub struct CachedXInputState {
    pub packet_number: u32,
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

impl From<&XInputState> for CachedXInputState {
    fn from(state: &XInputState) -> Self {
        Self {
            packet_number: state.raw.dwPacketNumber,
            buttons: state.raw.Gamepad.wButtons,
            left_trigger: state.left_trigger(),
            right_trigger: state.right_trigger(),
            thumb_lx: state.raw.Gamepad.sThumbLX,
            thumb_ly: state.raw.Gamepad.sThumbLY,
            thumb_rx: state.raw.Gamepad.sThumbRX,
            thumb_ry: state.raw.Gamepad.sThumbRY,
        }
    }
}

/// Convert XInput button state changes to GamepadEvents
///
/// Compares old and new button states and emits press/release events.
pub fn convert_xinput_buttons(
    old_buttons: Option<u16>,
    new_buttons: u16,
    prefix: &str,
) -> Vec<GamepadEvent> {
    let mut events = Vec::new();

    // Map XInput buttons to standardized names (matching gilrs convention)
    let button_mappings = [
        (button_flags::A, "a"),
        (button_flags::B, "b"),
        (button_flags::X, "x"),
        (button_flags::Y, "y"),
        (button_flags::LEFT_SHOULDER, "lb"),
        (button_flags::RIGHT_SHOULDER, "rb"),
        (button_flags::BACK, "minus"),
        (button_flags::START, "plus"),
        (button_flags::LEFT_THUMB, "l3"),
        (button_flags::RIGHT_THUMB, "r3"),
        (button_flags::DPAD_UP, "dpad.up"),
        (button_flags::DPAD_DOWN, "dpad.down"),
        (button_flags::DPAD_LEFT, "dpad.left"),
        (button_flags::DPAD_RIGHT, "dpad.right"),
    ];

    for (button_flag, name) in button_mappings {
        let old_pressed = old_buttons.is_some_and(|b| (b & button_flag) != 0);
        let new_pressed = (new_buttons & button_flag) != 0;

        if old_pressed != new_pressed {
            let control_id = if name.starts_with("dpad") {
                format!("{}.{}", prefix, name)
            } else {
                format!("{}.btn.{}", prefix, name)
            };

            events.push(GamepadEvent::Button {
                control_id,
                pressed: new_pressed,
            });
        }
    }

    events
}

/// Convert XInput analog axes to GamepadEvents
///
/// Compares old and new axis values and emits events for changed axes.
/// Applies radial deadzone and normalizes values to -1.0 to 1.0 range.
///
/// # Arguments
/// * `sequence_counter` - Mutable reference to monotonic sequence counter (prevents race conditions)
pub fn convert_xinput_axes(
    old_state: Option<&CachedXInputState>,
    new_state: &XInputState,
    prefix: &str,
    analog_config: Option<AnalogConfig>,
    sequence_counter: &mut u64,
) -> Vec<GamepadEvent> {
    let mut events = Vec::new();

    // Normalize sticks with radial deadzone (circular, not square)
    // XInput API spec recommends deadzone of 7849 for sticks
    const DEADZONE: f32 = 7849.0;

    let (lx, ly) = normalize_stick_radial(
        new_state.raw.Gamepad.sThumbLX,
        new_state.raw.Gamepad.sThumbLY,
        DEADZONE,
    );
    let (rx, ry) = normalize_stick_radial(
        new_state.raw.Gamepad.sThumbRX,
        new_state.raw.Gamepad.sThumbRY,
        DEADZONE,
    );

    // Normalize triggers (u8 0-255 → f32 -1.0 to 1.0)
    // Note: We map to full -1..1 range to match axis convention
    let lt = (new_state.left_trigger() as f32 / 255.0) * 2.0 - 1.0;
    let rt = (new_state.right_trigger() as f32 / 255.0) * 2.0 - 1.0;

    // Build axis list with change detection
    // Note: old_state values also need radial normalization for accurate change detection
    let (old_lx, old_ly) = old_state
        .map(|s| normalize_stick_radial(s.thumb_lx, s.thumb_ly, DEADZONE))
        .unwrap_or((0.0, 0.0));

    let (old_rx, old_ry) = old_state
        .map(|s| normalize_stick_radial(s.thumb_rx, s.thumb_ry, DEADZONE))
        .unwrap_or((0.0, 0.0));

    let axes = [
        ("lx", lx, old_state.map(|_| old_lx)),
        ("ly", -ly, old_state.map(|_| -old_ly)), // Invert Y
        ("rx", rx, old_state.map(|_| old_rx)),
        ("ry", -ry, old_state.map(|_| -old_ry)), // Invert Y
        (
            "zl",
            lt,
            old_state.map(|s| (s.left_trigger as f32 / 255.0) * 2.0 - 1.0),
        ),
        (
            "zr",
            rt,
            old_state.map(|s| (s.right_trigger as f32 / 255.0) * 2.0 - 1.0),
        ),
    ];

    for (axis_name, new_value, old_value) in axes {
        // Emit if changed from previous value (mapper will handle redundant event filtering)
        if old_value != Some(new_value) {
            *sequence_counter += 1;
            events.push(GamepadEvent::Axis {
                control_id: format!("{}.axis.{}", prefix, axis_name),
                value: new_value,
                analog_config: analog_config.clone(),
                sequence: *sequence_counter,
            });
        }
    }

    events
}

/// Poll XInput controller and return current state if available
///
/// # Returns
/// - `Ok(Some(state))` if controller is connected
/// - `Ok(None)` if controller is not connected
/// - `Err(_)` if XInput API failed
pub fn poll_xinput_controller(
    handle: &XInputHandle,
    user_index: u32,
) -> Result<Option<XInputState>, XInputUsageError> {
    match handle.get_state(user_index) {
        Ok(state) => Ok(Some(state)),
        Err(XInputUsageError::DeviceNotConnected) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: normalize_stick_radial tests are in the shared normalize module

    #[test]
    fn test_button_conversion() {
        // Press A button
        let events = convert_xinput_buttons(None, button_flags::A, "gamepad1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            GamepadEvent::Button {
                control_id,
                pressed,
            } => {
                assert_eq!(control_id, "gamepad1.btn.a");
                assert_eq!(*pressed, true);
            },
            _ => panic!("Expected button event"),
        }
    }

    #[test]
    fn test_button_no_change() {
        // Same state, no events
        let events = convert_xinput_buttons(Some(button_flags::A), button_flags::A, "gamepad1");
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_dpad_format() {
        // D-Pad uses different format
        let events = convert_xinput_buttons(None, button_flags::DPAD_UP, "gamepad1");
        assert_eq!(events.len(), 1);
        match &events[0] {
            GamepadEvent::Button { control_id, .. } => {
                assert_eq!(control_id, "gamepad1.dpad.up");
            },
            _ => panic!("Expected button event"),
        }
    }
}
