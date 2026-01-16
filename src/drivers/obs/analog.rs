//! Analog input processing for gamepad control
//!
//! Handles velocity-based pan/zoom control using gamepad analog sticks.
//! Applies gamma curves for finer control and manages a 60Hz timer for smooth motion.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, trace, warn};

use super::driver::ObsDriver;

/// Shape analog input with gamma curve
///
/// Applies gamma curve for finer control near center.
/// Note: Deadzone is already applied upstream by the gamepad mapper.
/// Formula:
/// 1. Extract sign and magnitude
/// 2. Apply gamma curve: shaped = magnitude^gamma
/// 3. Return sign × shaped
pub(super) fn shape_analog(value: f64, gamma: f64) -> f64 {
    // Return 0 if not finite
    if !value.is_finite() {
        return 0.0;
    }

    let sign = if value >= 0.0 { 1.0 } else { -1.0 };
    let magnitude = value.abs().min(1.0).max(0.0);

    // Apply gamma curve for finer control at low values
    let shaped = magnitude.powf(gamma);

    sign * shaped
}

/// Velocity state for analog motion (per scene/source)
#[derive(Debug, Clone, Default)]
pub(super) struct AnalogRate {
    pub(super) scene: String,
    pub(super) source: String,
    pub(super) vx: f64,  // pixels per tick (at 60Hz)
    pub(super) vy: f64,  // pixels per tick
    pub(super) vs: f64,  // scale delta per tick
}

impl ObsDriver {
    /// Set analog velocity for a scene/source
    pub(super) fn set_analog_rate(&self, scene_name: &str, source_name: &str, vx: Option<f64>, vy: Option<f64>, vs: Option<f64>) {
        let cache_key = self.cache_key(scene_name, source_name);

        let mut rates = self.analog_rates.write();

        // Get existing rate or create default
        let current = rates.get(&cache_key).cloned().unwrap_or_else(|| AnalogRate {
            scene: scene_name.to_string(),
            source: source_name.to_string(),
            vx: 0.0,
            vy: 0.0,
            vs: 0.0,
        });

        // Apply partial updates (only update provided values)
        let new_vx = vx.unwrap_or(current.vx);
        let new_vy = vy.unwrap_or(current.vy);
        let new_vs = vs.unwrap_or(current.vs);

        debug!(
            "OBS analog rate: {}/{} → vx={:.3} ({}), vy={:.3} ({}), vs={:.3} ({})",
            scene_name, source_name,
            new_vx, if vx.is_some() { "new" } else { "keep" },
            new_vy, if vy.is_some() { "new" } else { "keep" },
            new_vs, if vs.is_some() { "new" } else { "keep" }
        );

        if new_vx == 0.0 && new_vy == 0.0 && new_vs == 0.0 {
            // Remove entry if all velocities are zero
            rates.remove(&cache_key);
            // Clear error count when rate is removed
            self.analog_error_count.write().remove(&cache_key);
        } else {
            // Update or insert velocity with merged values
            rates.insert(cache_key.clone(), AnalogRate {
                scene: scene_name.to_string(),
                source: source_name.to_string(),
                vx: new_vx,
                vy: new_vy,
                vs: new_vs,
            });
            // Clear error count when rate is updated (fresh start)
            self.analog_error_count.write().remove(&cache_key);
        }

        // Manage timer based on active rates
        if rates.is_empty() {
            self.stop_analog_timer();
        } else {
            self.ensure_analog_timer();
        }
    }

    /// Start analog motion timer at ~60Hz if not already running
    pub(super) fn ensure_analog_timer(&self) {
        let mut active = self.analog_timer_active.lock();
        if *active {
            return; // Already running
        }

        *active = true;
        *self.last_analog_tick.lock() = Instant::now();

        // Spawn timer task
        let rates = Arc::clone(&self.analog_rates);
        let last_tick = Arc::clone(&self.last_analog_tick);
        let timer_active = Arc::clone(&self.analog_timer_active);
        let driver_self = Arc::new(self.clone_for_timer());

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(16)); // ~60Hz
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                // Check if timer should stop
                if !*timer_active.lock() {
                    debug!("OBS analog timer stopped");
                    break;
                }

                // Calculate dt (normalized to 60Hz)
                let now = Instant::now();
                let interval_ms = {
                    let mut last = last_tick.lock();
                    let elapsed = now.duration_since(*last).as_millis() as f64;
                    *last = now;
                    elapsed
                };
                let dt = interval_ms / 16.0; // Normalize to 60Hz

                // Process all active rates
                let rates_snapshot: Vec<AnalogRate> = {
                    let r = rates.read();
                    r.values().cloned().collect()
                };

                for rate in rates_snapshot {
                    let dx = rate.vx * dt;
                    let dy = rate.vy * dt;
                    let ds = rate.vs * dt;

                    if dx != 0.0 || dy != 0.0 || ds != 0.0 {
                        let dx_opt = if dx != 0.0 { Some(dx) } else { None };
                        let dy_opt = if dy != 0.0 { Some(dy) } else { None };
                        let ds_opt = if ds != 0.0 { Some(ds) } else { None };

                        let cache_key = driver_self.cache_key(&rate.scene, &rate.source);

                        match driver_self.apply_delta(
                            &rate.scene,
                            &rate.source,
                            dx_opt,
                            dy_opt,
                            ds_opt
                        ).await {
                            Ok(_) => {
                                // Success - clear error count
                                driver_self.analog_error_count.write().remove(&cache_key);
                            }
                            Err(e) => {
                                // Increment error count
                                let mut error_counts = driver_self.analog_error_count.write();
                                let count = error_counts.entry(cache_key.clone()).or_insert(0);
                                *count += 1;

                                const MAX_RETRIES: usize = 3;
                                if *count >= MAX_RETRIES {
                                    // Remove the failing rate to prevent infinite loop
                                    drop(error_counts); // Release lock before modifying rates
                                    driver_self.analog_rates.write().remove(&cache_key);
                                    warn!("OBS analog tick: removed failing rate '{}' after {} attempts. Last error: {}",
                                        cache_key, MAX_RETRIES, e);
                                } else {
                                    // Only trace on early attempts
                                    trace!("OBS analog tick error (attempt {}): {}", count, e);
                                }
                            }
                        }
                    }
                }

                // Check if all rates are now zero (stop timer)
                if rates.read().is_empty() {
                    *timer_active.lock() = false;
                }
            }
        });

        debug!("OBS analog timer started at ~60Hz");
    }

    /// Stop the analog motion timer
    pub(super) fn stop_analog_timer(&self) {
        *self.analog_timer_active.lock() = false;
    }
}

