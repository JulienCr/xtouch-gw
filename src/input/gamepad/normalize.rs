//! Shared normalization functions for gamepad stick and trigger values.
//!
//! This module provides the canonical implementations of stick normalization
//! for both XInput and gilrs backends, ensuring consistent behavior across
//! all input sources.
//!
//! # Stick Normalization
//!
//! Uses radial (circular) deadzone rather than per-axis (square) deadzone.
//! This ensures diagonal movements can reach full magnitude (1.0) and provides
//! consistent response regardless of direction.
//!
//! # Key Functions
//!
//! - [`normalize_stick_radial`]: For raw XInput values (i16 range)
//! - [`square_to_circle`]: For gilrs values (already normalized to -1.0..1.0)

/// XInput left thumbstick deadzone radius.
///
/// Values from Microsoft's XInput documentation.
pub const XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE: i16 = 7849;

/// XInput right thumbstick deadzone radius.
///
/// Right stick has a slightly larger deadzone than left.
pub const XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE: i16 = 8689;

/// XInput trigger threshold below which input is ignored.
///
/// Triggers report 0-255; values below this threshold are treated as zero.
pub const XINPUT_GAMEPAD_TRIGGER_THRESHOLD: u8 = 30;

/// Normalize XInput stick with radial deadzone and radial scaling.
///
/// Uses circular deadzone (not square) and ensures diagonal movements reach magnitude 1.0.
/// This fixes the issue where per-axis normalization caused diagonals to only reach ~0.87.
///
/// # Arguments
/// * `raw_x`, `raw_y` - Raw stick values from XInput (-32768 to 32767)
/// * `deadzone` - Circular deadzone radius (7849 for left stick, 8689 for right stick)
///
/// # Returns
/// * `(norm_x, norm_y)` - Normalized values in [-1.0, 1.0] with magnitude <= 1.0
///
/// # Example
/// ```
/// use xtouch_gw::input::gamepad::normalize::{
///     normalize_stick_radial, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE
/// };
///
/// // Centered stick returns zero
/// let (x, y) = normalize_stick_radial(0, 0, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
/// assert_eq!((x, y), (0.0, 0.0));
///
/// // Full right returns ~1.0
/// let (x, y) = normalize_stick_radial(32767, 0, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
/// assert!(x > 0.99);
/// ```
pub fn normalize_stick_radial(raw_x: i16, raw_y: i16, deadzone: f32) -> (f32, f32) {
    let x = raw_x as f32;
    let y = raw_y as f32;
    let magnitude = (x * x + y * y).sqrt();

    if magnitude <= deadzone {
        return (0.0, 0.0);
    }

    // Maximum single-axis deflection (NOT diagonal!)
    // Use 32768.0 to handle i16::MIN (-32768) correctly
    const MAX_MAGNITUDE: f32 = 32768.0;

    if deadzone >= MAX_MAGNITUDE {
        return (0.0, 0.0);
    }

    // Radial rescaling: map [deadzone, max_magnitude] -> [0, 1]
    // Diagonals may exceed 1.0 before clamping (expected and correct)
    let normalized_magnitude = ((magnitude - deadzone) / (MAX_MAGNITUDE - deadzone)).min(1.0);
    let scale = normalized_magnitude / magnitude;

    (x * scale, y * scale)
}

/// Gilrs normalization mode selection.
///
/// Different gamepads report stick values differently:
/// - Some form a square (corners reach 1,1)
/// - Some form a concave diamond/astroid (diagonals pulled inward)
/// - Some form a circle (already normalized)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GilrsNormMode {
    /// No transformation - pass through raw values, only clamp if magnitude > 1.
    /// Use if raw values already form a circle.
    RadialClamp,

    /// Square to circle - shrink diagonals. Use if raw values form a square
    /// (corners reaching 1,1 with magnitude 1.414).
    /// At (1, 1): output (0.707, 0.707), magnitude 1.0
    SquareToCircle,

    /// Astroid/diamond to circle - expand diagonals. Use if raw values form
    /// a concave diamond (diagonals pulled inward, magnitude < 1).
    /// Scales up diagonal values to reach magnitude 1.0.
    AstroidToCircle,
}

/// Current normalization mode for gilrs controllers.
/// Change this to test different transformations.
pub const GILRS_NORM_MODE: GilrsNormMode = GilrsNormMode::SquareToCircle;

/// Map square input to circular output (radial normalization for gilrs).
///
/// Gilrs provides per-axis values in [-1.0, 1.0] forming a square.
/// This maps the square to a circle by scaling based on direction:
/// - Points on the edge of the square map to the edge of the circle (magnitude 1.0)
/// - Interior points scale proportionally
///
/// Formula: scale = max(|x|, |y|) / magnitude
///
/// # Examples
/// - Full up (0, 1): already on circle edge → (0, 1) mag 1.0
/// - Diagonal (1, 1): corner of square → (0.707, 0.707) mag 1.0
/// - Half diagonal (0.5, 0.5): → (0.354, 0.354) mag 0.5
///
/// # Arguments
/// * `x`, `y` - Stick values from gilrs (-1.0 to 1.0 per axis)
///
/// # Returns
/// * `(x, y)` - Values mapped to unit circle with magnitude in [0.0, 1.0]
pub fn square_to_circle(x: f32, y: f32) -> (f32, f32) {
    let magnitude = (x * x + y * y).sqrt();

    if magnitude < 0.0001 {
        return (0.0, 0.0);
    }

    // Distance to edge of square in this direction
    let max_axis = x.abs().max(y.abs());

    // Scale factor: maps square edge to circle edge
    let scale = max_axis / magnitude;

    (x * scale, y * scale)
}

/// Clamp input to unit circle (radial clamp for gilrs).
///
/// Only modifies positions outside the unit circle by scaling them back
/// to magnitude 1.0. Interior positions are preserved exactly.
///
/// This matches XInput behavior more closely for intermediate values:
/// - At (0.5, 0.5): magnitude = 0.707, output = (0.5, 0.5) unchanged
/// - At (1, 1): magnitude = 1.414 > 1, output = (0.707, 0.707) clamped
///
/// # Arguments
/// * `x`, `y` - Stick values from gilrs (-1.0 to 1.0 per axis)
///
/// # Returns
/// * `(x, y)` - Values clamped to unit circle
pub fn radial_clamp(x: f32, y: f32) -> (f32, f32) {
    let magnitude = (x * x + y * y).sqrt();

    if magnitude <= 1.0 {
        // Inside unit circle: keep as-is
        (x, y)
    } else {
        // Outside unit circle: scale back to edge
        (x / magnitude, y / magnitude)
    }
}

/// Map astroid/concave diamond input to circular output.
///
/// Use this when raw gilrs values form a concave diamond (diagonals pulled
/// inward toward center). This is the INVERSE of square_to_circle.
///
/// Expands diagonal values outward so they reach magnitude 1.0:
/// - At (1, 0): output (1, 0), magnitude 1.0 (unchanged)
/// - At (0.6, 0.6): if this is "full diagonal", expand to (0.707, 0.707), magnitude 1.0
///
/// Formula: scale = magnitude / max(|x|, |y|)
/// This is the inverse of square_to_circle's scale = max(|x|, |y|) / magnitude
///
/// # Arguments
/// * `x`, `y` - Stick values from gilrs (-1.0 to 1.0 per axis)
///
/// # Returns
/// * `(x, y)` - Values expanded to unit circle
pub fn astroid_to_circle(x: f32, y: f32) -> (f32, f32) {
    let magnitude = (x * x + y * y).sqrt();

    if magnitude < 0.0001 {
        return (0.0, 0.0);
    }

    let max_axis = x.abs().max(y.abs());

    if max_axis < 0.0001 {
        return (0.0, 0.0);
    }

    // Inverse of square_to_circle: expand diagonals outward
    // scale = magnitude / max_axis (instead of max_axis / magnitude)
    let scale = magnitude / max_axis;

    // Clamp to unit circle in case expansion overshoots
    let out_x = x * scale;
    let out_y = y * scale;
    let out_mag = (out_x * out_x + out_y * out_y).sqrt();

    if out_mag > 1.0 {
        (out_x / out_mag, out_y / out_mag)
    } else {
        (out_x, out_y)
    }
}

/// Apply gilrs radial normalization based on GILRS_NORM_MODE.
///
/// This is the main entry point for gilrs stick normalization.
/// Uses the transformation selected by the GILRS_NORM_MODE constant.
pub fn normalize_gilrs_stick(x: f32, y: f32) -> (f32, f32) {
    match GILRS_NORM_MODE {
        GilrsNormMode::RadialClamp => radial_clamp(x, y),
        GilrsNormMode::SquareToCircle => square_to_circle(x, y),
        GilrsNormMode::AstroidToCircle => astroid_to_circle(x, y),
    }
}

/// Normalize XInput trigger value (u8) to 0.0 to 1.0.
///
/// Applies the trigger threshold deadzone and scales the remaining range
/// to the full 0.0-1.0 output range.
///
/// # Arguments
/// * `value` - Raw trigger value from XInput (0-255)
///
/// # Returns
/// * Normalized value in [0.0, 1.0]
///
/// # Example
/// ```
/// use xtouch_gw::input::gamepad::normalize::normalize_trigger;
///
/// assert_eq!(normalize_trigger(0), 0.0);
/// assert_eq!(normalize_trigger(29), 0.0); // Below threshold
/// assert!(normalize_trigger(255) > 0.99);
/// ```
pub fn normalize_trigger(value: u8) -> f32 {
    if value < XINPUT_GAMEPAD_TRIGGER_THRESHOLD {
        return 0.0;
    }
    let adjusted = value - XINPUT_GAMEPAD_TRIGGER_THRESHOLD;
    let range = 255 - XINPUT_GAMEPAD_TRIGGER_THRESHOLD;
    adjusted as f32 / range as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_stick_radial_centered() {
        let (x, y) = normalize_stick_radial(0, 0, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn test_normalize_stick_radial_inside_deadzone() {
        // Inside deadzone on X axis only
        let (x, y) = normalize_stick_radial(7000, 0, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn test_normalize_stick_radial_outside_deadzone_diagonally() {
        // 7000^2 + 7000^2 = ~9899 magnitude > 7849, so NOT in deadzone
        let (x, y) = normalize_stick_radial(7000, 7000, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
        assert!(x > 0.0 && y > 0.0);
    }

    #[test]
    fn test_normalize_stick_radial_full_right() {
        let (x, y) = normalize_stick_radial(32767, 0, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
        assert!((x - 1.0).abs() < 0.01);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn test_normalize_stick_radial_full_left() {
        let (x, y) = normalize_stick_radial(-32768, 0, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32);
        assert!((x + 1.0).abs() < 0.01);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn test_square_to_circle_cardinal() {
        // Full up: already at circle edge
        let (x, y) = square_to_circle(0.0, 1.0);
        assert!((x - 0.0).abs() < 0.001);
        assert!((y - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_square_to_circle_diagonal() {
        // Diagonal (1, 1): corner of square maps to circle edge
        let (x, y) = square_to_circle(1.0, 1.0);
        let mag = (x * x + y * y).sqrt();
        // Magnitude should be ~1.0
        assert!((mag - 1.0).abs() < 0.01, "Diagonal magnitude was {}", mag);
    }

    #[test]
    fn test_square_to_circle_center() {
        let (x, y) = square_to_circle(0.0, 0.0);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn test_normalize_trigger() {
        assert_eq!(normalize_trigger(0), 0.0);
        assert_eq!(normalize_trigger(29), 0.0);
        assert_eq!(normalize_trigger(30), 0.0); // At threshold, still zero
        assert!(normalize_trigger(31) > 0.0); // Just above threshold
        assert!(normalize_trigger(255) > 0.99);
    }
}
