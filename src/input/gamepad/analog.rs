//! Analog axis processing (deadzone, gamma, normalization)

use crate::config::AnalogConfig;

/// Process an analog axis value through deadzone, gamma, and normalization
///
/// # Arguments
/// * `raw_value` - Raw axis value from gilrs (-1.0 to 1.0)
/// * `config` - Analog configuration (deadzone, gamma)
///
/// # Returns
/// Processed value in range -1.0 to 1.0, or None if within deadzone
pub fn process_axis(raw_value: f32, config: &AnalogConfig) -> Option<f32> {
    // Apply deadzone - ignore small movements
    if raw_value.abs() < config.deadzone {
        return None;
    }

    // Normalize to account for deadzone
    // Map [deadzone..1.0] → [0.0..1.0]
    let sign = raw_value.signum();
    let magnitude = raw_value.abs();
    let normalized = (magnitude - config.deadzone) / (1.0 - config.deadzone);

    // Apply gamma curve for sensitivity adjustment
    // gamma > 1.0 = more precise at center, less at edges
    // gamma < 1.0 = less precise at center, more at edges
    let curved = normalized.powf(config.gamma);

    // Restore sign
    Some(sign * curved)
}

/// Apply axis inversion if configured
///
/// # Arguments
/// * `value` - Processed axis value
/// * `axis_id` - Axis identifier (e.g., "lx", "ly", "rx", "ry")
/// * `config` - Analog configuration with invert map
///
/// # Returns
/// Value with inversion applied if configured
pub fn apply_inversion(value: f32, axis_id: &str, config: &AnalogConfig) -> f32 {
    if config.invert.get(axis_id).copied().unwrap_or(false) {
        -value
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_config() -> AnalogConfig {
        AnalogConfig {
            pan_gain: 15.0,
            zoom_gain: 3.0,
            deadzone: 0.02,
            gamma: 1.5,
            invert: HashMap::new(),
        }
    }

    #[test]
    fn test_deadzone_filters_small_values() {
        let config = test_config();

        // Within deadzone - should return None
        assert_eq!(process_axis(0.01, &config), None);
        assert_eq!(process_axis(-0.01, &config), None);

        // Outside deadzone - should return Some
        assert!(process_axis(0.5, &config).is_some());
        assert!(process_axis(-0.5, &config).is_some());
    }

    #[test]
    fn test_gamma_curve() {
        let config = test_config();

        // At max, should still be near 1.0
        let result = process_axis(1.0, &config).unwrap();
        assert!((result - 1.0).abs() < 0.1);

        // At center (after deadzone), should be less than linear
        let result = process_axis(0.5, &config).unwrap();
        assert!(result < 0.5);  // gamma > 1 reduces mid values
    }

    #[test]
    fn test_inversion() {
        let mut config = test_config();
        config.invert.insert("ly".to_string(), true);

        assert_eq!(apply_inversion(0.5, "lx", &config), 0.5);   // Not inverted
        assert_eq!(apply_inversion(0.5, "ly", &config), -0.5);  // Inverted
        assert_eq!(apply_inversion(-0.5, "ly", &config), 0.5);  // Inverted
    }
}
