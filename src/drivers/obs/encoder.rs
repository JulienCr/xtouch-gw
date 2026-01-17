//! Encoder acceleration tracking
//!
//! Tracks encoder rotation velocity and applies adaptive acceleration multipliers
//! for fast movements using Exponential Moving Average (EMA).

use std::collections::HashMap;
use std::time::Instant;

/// Encoder speed tracking state (per encoder)
#[derive(Debug, Clone)]
pub(super) struct EncoderState {
    pub(super) last_ts: Option<Instant>,
    pub(super) velocity_ema: f64,
    pub(super) last_direction: i8,
}

/// Encoder speed tracker with adaptive acceleration
///
/// Tracks encoder rotation velocity and applies acceleration multipliers
/// for fast movements. Uses Exponential Moving Average (EMA) for smooth
/// velocity tracking.
#[derive(Debug, Clone)]
pub(super) struct EncoderSpeedTracker {
    // EMA smoothing weight (0-1, higher = more responsive)
    ema_alpha: f64,
    // Reference velocity in ticks/sec for acceleration calculation
    accel_vref: f64,
    // Acceleration coefficient
    accel_k: f64,
    // Acceleration curve exponent
    accel_gamma: f64,
    // Maximum acceleration multiplier
    max_multiplier: f64,
    // Minimum interval between ticks to count (ms)
    min_interval_ms: u64,
    // Damping factor on direction change
    direction_flip_dampen: f64,
    // Idle time before resetting EMA (ms)
    idle_reset_ms: u64,
    // Per-encoder state
    states: HashMap<String, EncoderState>,
}

impl EncoderSpeedTracker {
    /// Create with default parameters (matching TypeScript implementation)
    pub(super) fn new() -> Self {
        Self {
            ema_alpha: 0.75,
            accel_vref: 9.0,
            accel_k: 3.9,
            accel_gamma: 1.4,
            max_multiplier: 15.0,
            min_interval_ms: 4,
            direction_flip_dampen: 0.5,
            idle_reset_ms: 700,
            states: HashMap::new(),
        }
    }

    /// Track an encoder event and return acceleration multiplier
    ///
    /// Returns the acceleration factor to apply to base_delta.
    /// Example: track_event("vpot1", 1.0) → 3.5 (multiply base delta by 3.5x)
    pub(super) fn track_event(&mut self, encoder_id: &str, base_delta: f64) -> f64 {
        let direction = base_delta.signum() as i8;
        let now = Instant::now();

        // Get or create state
        let state = self
            .states
            .entry(encoder_id.to_string())
            .or_insert(EncoderState {
                last_ts: None,
                velocity_ema: 0.0,
                last_direction: 0,
            });

        // Calculate instantaneous velocity (ticks per second)
        if let Some(last_ts) = state.last_ts {
            if base_delta != 0.0 {
                let interval_ms = now.duration_since(last_ts).as_millis() as u64;

                if interval_ms >= self.min_interval_ms {
                    let inst_velocity = 1000.0 / interval_ms.max(1) as f64;

                    // Update EMA (Exponential Moving Average)
                    let is_bootstrap =
                        state.velocity_ema == 0.0 || interval_ms > self.idle_reset_ms;

                    state.velocity_ema = if is_bootstrap {
                        inst_velocity
                    } else {
                        self.ema_alpha * inst_velocity + (1.0 - self.ema_alpha) * state.velocity_ema
                    };
                }
            }
        }

        // Update timestamp if non-zero delta
        if base_delta != 0.0 {
            state.last_ts = Some(now);
        }

        // Calculate acceleration multiplier
        let v_norm = state.velocity_ema.max(0.0) / self.accel_vref;
        let mut accel = 1.0 + self.accel_k * v_norm.powf(self.accel_gamma);
        accel = accel.max(1.0).min(self.max_multiplier);

        // Dampen on direction flip
        if base_delta != 0.0
            && state.last_direction != 0
            && direction != 0
            && direction != state.last_direction
        {
            accel *= self.direction_flip_dampen;
        }

        // Update direction
        if base_delta != 0.0 && direction != 0 {
            state.last_direction = direction;
        }

        accel
    }
}
