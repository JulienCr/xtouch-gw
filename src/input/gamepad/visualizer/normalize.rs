//! Re-exports normalization functions from the shared module.
//!
//! This module re-exports normalization utilities from the central
//! `gamepad::normalize` module, providing backwards compatibility
//! for code that imports from `visualizer::normalize`.

pub use crate::input::gamepad::normalize::{
    normalize_gilrs_stick, normalize_stick_radial, normalize_trigger,
    XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE, XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE,
};

// Allow unused re-exports for potential external use
#[allow(unused_imports)]
pub use crate::input::gamepad::normalize::{
    astroid_to_circle, radial_clamp, square_to_circle, GilrsNormMode, GILRS_NORM_MODE,
};

// Re-export trigger threshold for backwards compatibility (currently unused)
#[allow(unused_imports)]
pub use crate::input::gamepad::normalize::XINPUT_GAMEPAD_TRIGGER_THRESHOLD;
