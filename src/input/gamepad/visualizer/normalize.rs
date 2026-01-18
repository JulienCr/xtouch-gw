//! Re-exports normalization functions from the shared module.
//!
//! This module re-exports normalization utilities from the central
//! `gamepad::normalize` module for use by the visualizer components.

pub use crate::input::gamepad::normalize::{
    normalize_gilrs_stick, normalize_stick_radial, normalize_trigger,
    XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE, XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE,
};
