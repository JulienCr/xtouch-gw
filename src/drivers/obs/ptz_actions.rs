//! PTZ reset actions for OBS
//!
//! Handles `resetPosition` and `resetZoom` action execution.

use anyhow::{Context, Result};
use serde_json::Value;
use tracing::{debug, info};

use super::analog::shape_analog;
use super::camera_actions::PtzTargetContext;
use super::driver::ObsDriver;

/// PTZ axis for nudge/scale operations
#[derive(Debug, Clone, Copy)]
pub(super) enum PtzAxis {
    X,
    Y,
    Scale,
}

impl ObsDriver {
    /// Convert a MIDI encoder value into a signed directional delta for PTZ operations.
    ///
    /// Standard encoder values: 0/64=no change, 1-63=clockwise, 65-127=counter-clockwise.
    ///
    /// # Arguments
    /// * `value` - The MIDI encoder value as a JSON Value
    /// * `step` - The amount to move per encoder tick
    ///
    /// # Returns
    /// Returns `step` for clockwise input, `-step` for counter-clockwise, or `0.0` for no movement.
    pub(super) fn encoder_value_to_delta(value: &Value, step: f64) -> f64 {
        let v = match value.as_f64() {
            Some(v) => v,
            None => return 0.0,
        };

        if (1.0..=63.0).contains(&v) {
            step
        } else if (65.0..=127.0).contains(&v) {
            -step
        } else {
            0.0
        }
    }

    /// Handle gamepad analog input for PTZ operations.
    ///
    /// Processes raw gamepad value with gamma shaping and gain scaling,
    /// then sets the appropriate analog rate for continuous movement.
    /// This function updates the internal analog rate state directly and does not return a value.
    pub(super) fn handle_gamepad_analog(
        &self,
        value: Option<&Value>,
        scene: &str,
        source: &str,
        step: f64,
        gain: f64,
        axis: PtzAxis,
    ) {
        let raw_value = match value {
            Some(Value::Number(n)) => n.as_f64(),
            _ => None,
        };

        if let Some(v) = raw_value {
            let clamped = v.clamp(-1.0, 1.0);
            let gamma = *self.analog_gamma.read();
            let shaped = shape_analog(clamped, gamma);
            let velocity = shaped * step * gain;

            match axis {
                PtzAxis::X => self.set_analog_rate(scene, source, Some(velocity), None, None),
                PtzAxis::Y => self.set_analog_rate(scene, source, None, Some(velocity), None),
                PtzAxis::Scale => self.set_analog_rate(scene, source, None, None, Some(velocity)),
            }
        }
    }

    /// Handle encoder input for PTZ operations with acceleration.
    ///
    /// Processes encoder delta with acceleration tracking and applies
    /// the resulting delta to the scene transform.
    pub(super) async fn handle_encoder_ptz(
        &self,
        ctx: &super::super::ExecutionContext,
        scene: &str,
        source: &str,
        step: f64,
        axis: PtzAxis,
    ) -> Result<()> {
        let delta = match &ctx.value {
            Some(value) => Self::encoder_value_to_delta(value, step),
            None => step,
        };

        if delta == 0.0 {
            return Ok(());
        }

        let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
        let accel = self.encoder_tracker.lock().track_event(control_id, delta);
        let final_delta = delta * accel;

        debug!(
            "OBS {:?} encoder: id='{}' delta={} accel={:.2}x final={:.2}",
            axis, control_id, delta, accel, final_delta
        );

        match axis {
            PtzAxis::X => {
                self.apply_delta(scene, source, Some(final_delta), None, None)
                    .await
            },
            PtzAxis::Y => {
                self.apply_delta(scene, source, None, Some(final_delta), None)
                    .await
            },
            PtzAxis::Scale => {
                self.apply_delta(scene, source, None, None, Some(final_delta))
                    .await
            },
        }
    }

    /// Execute a PTZ nudge/scale action (common logic for nudgeX, nudgeY, scaleUniform).
    pub(super) async fn execute_ptz_action(
        &self,
        params: &[Value],
        ctx: &super::super::ExecutionContext,
        axis: PtzAxis,
        default_step: f64,
    ) -> Result<()> {
        let (scene_name, source_name, step) = self
            .parse_camera_params_with_step(params)
            .unwrap_or_else(|_| {
                let scene = params
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let source = params
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (scene, source, default_step)
            });

        if !self.is_ptz_enabled_by_scene(&scene_name) {
            return Ok(());
        }

        let is_gamepad = PtzTargetContext::from_ctx(ctx).is_gamepad();

        if is_gamepad {
            let gain = match axis {
                PtzAxis::X | PtzAxis::Y => *self.analog_pan_gain.read(),
                PtzAxis::Scale => *self.analog_zoom_gain.read(),
            };
            self.handle_gamepad_analog(
                ctx.value.as_ref(),
                &scene_name,
                &source_name,
                step,
                gain,
                axis,
            );
        } else {
            self.handle_encoder_ptz(ctx, &scene_name, &source_name, step, axis)
                .await?;
        }

        Ok(())
    }

    /// Execute the `resetPosition` action.
    ///
    /// Resets a scene item's position to the center of the canvas.
    pub(super) async fn execute_reset_position(&self, params: &[Value]) -> Result<()> {
        let (scene_name, source_name) = self
            .parse_camera_params(params)
            .context("resetPosition requires [camera_id] or [scene, source]")?;

        if !self.is_ptz_enabled_by_scene(&scene_name) {
            return Ok(());
        }

        info!(
            "OBS Reset position: scene='{}' source='{}'",
            scene_name, source_name
        );

        // Stop any analog motion on position axes
        self.set_analog_rate(&scene_name, &source_name, Some(0.0), Some(0.0), None);

        // Resolve item ID
        let item_id = self.resolve_item_id(&scene_name, &source_name).await?;

        // Get canvas dimensions
        let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;

        // Build transform to reset position to center
        let mut transform = obws::requests::scene_items::SceneItemTransform::default();

        // Set alignment to center (so position refers to object center, not corner)
        transform.alignment = Some(obws::common::Alignment::CENTER);

        // Set position to canvas center
        transform.position = Some(obws::requests::scene_items::Position {
            x: Some((canvas_width / 2.0) as f32),
            y: Some((canvas_height / 2.0) as f32),
            ..Default::default()
        });

        // Send to OBS
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        client
            .scene_items()
            .set_transform(obws::requests::scene_items::SetTransform {
                scene: &scene_name,
                item_id,
                transform,
            })
            .await
            .context("Failed to reset scene item position")?;

        // Update cache with new position and alignment
        let cache_key = self.cache_key(&scene_name, &source_name);
        if let Some(state) = self.transform_cache.write().get_mut(&cache_key) {
            state.x = canvas_width / 2.0;
            state.y = canvas_height / 2.0;
            state.alignment = 0; // CENTER
        }

        debug!(
            "OBS reset position: '{}' position=({:.1},{:.1}) alignment=CENTER",
            self.cache_key(&scene_name, &source_name),
            canvas_width / 2.0,
            canvas_height / 2.0
        );

        Ok(())
    }

    /// Execute the `resetZoom` action.
    ///
    /// Resets a scene item's zoom to default (bounds-based or scale-based).
    pub(super) async fn execute_reset_zoom(&self, params: &[Value]) -> Result<()> {
        let (scene_name, source_name) = self
            .parse_camera_params(params)
            .context("resetZoom requires [camera_id] or [scene, source]")?;

        if !self.is_ptz_enabled_by_scene(&scene_name) {
            return Ok(());
        }

        info!(
            "OBS Reset zoom: scene='{}' source='{}'",
            scene_name, source_name
        );

        // Stop any analog motion on zoom axis
        self.set_analog_rate(&scene_name, &source_name, None, None, Some(0.0));

        // Resolve item ID
        let item_id = self.resolve_item_id(&scene_name, &source_name).await?;

        // Read current transform from OBS (not cache!) to determine type
        let current = self.read_transform(&scene_name, item_id).await?;

        // Detect if this source uses bounds-based or scale-based transform
        let is_bounds_based = matches!(
            (current.bounds_width, current.bounds_height),
            (Some(bw), Some(bh)) if bw > 0.0 && bh > 0.0
        );

        // Build transform to reset zoom
        let mut transform = obws::requests::scene_items::SceneItemTransform::default();

        if is_bounds_based {
            // For bounds-based sources (NDI cameras): reset bounds to canvas dimensions
            let (canvas_width, canvas_height) = self.get_canvas_dimensions().await?;

            transform.bounds = Some(obws::requests::scene_items::Bounds {
                width: Some(canvas_width as f32),
                height: Some(canvas_height as f32),
                ..Default::default()
            });

            info!(
                "OBS Reset zoom (bounds): {}x{}",
                canvas_width as u32, canvas_height as u32
            );
        } else {
            // For scale-based sources: reset scale to 1.0
            transform.scale = Some(obws::requests::scene_items::Scale {
                x: Some(1.0),
                y: Some(1.0),
                ..Default::default()
            });
            debug!("OBS Reset zoom (scale): 1.0x1.0");
        }

        // Send to OBS
        let guard = self.client.read().await;
        let client = guard.as_ref().context("OBS client not connected")?;

        client
            .scene_items()
            .set_transform(obws::requests::scene_items::SetTransform {
                scene: &scene_name,
                item_id,
                transform,
            })
            .await
            .context("Failed to reset scene item zoom")?;

        // Invalidate cache to force fresh read on next operation
        let cache_key = self.cache_key(&scene_name, &source_name);
        self.transform_cache.write().remove(&cache_key);

        debug!("OBS reset zoom: '{}' (cache invalidated)", cache_key);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Test encoder_value_to_delta with various MIDI encoder values.
    /// Standard encoder protocol: 0/64=no change, 1-63=clockwise, 65-127=counter-clockwise
    mod encoder_value_to_delta_tests {
        use super::*;

        #[test]
        fn zero_returns_no_change() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(0.0), 2.0), 0.0);
        }

        #[test]
        fn center_64_returns_no_change() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(64.0), 2.0), 0.0);
        }

        #[test]
        fn clockwise_min_1_returns_positive_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(1.0), 2.0), 2.0);
        }

        #[test]
        fn clockwise_max_63_returns_positive_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(63.0), 2.0), 2.0);
        }

        #[test]
        fn clockwise_mid_32_returns_positive_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(32.0), 5.0), 5.0);
        }

        #[test]
        fn counter_clockwise_min_65_returns_negative_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(65.0), 2.0), -2.0);
        }

        #[test]
        fn counter_clockwise_max_127_returns_negative_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(127.0), 2.0), -2.0);
        }

        #[test]
        fn counter_clockwise_mid_96_returns_negative_step() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(96.0), 3.0), -3.0);
        }

        #[test]
        fn out_of_range_negative_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(-1.0), 2.0), 0.0);
        }

        #[test]
        fn out_of_range_above_127_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(128.0), 2.0), 0.0);
        }

        #[test]
        fn string_value_returns_zero() {
            assert_eq!(
                ObsDriver::encoder_value_to_delta(&json!("invalid"), 2.0),
                0.0
            );
        }

        #[test]
        fn null_value_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&Value::Null, 2.0), 0.0);
        }

        #[test]
        fn boolean_value_returns_zero() {
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(true), 2.0), 0.0);
        }

        #[test]
        fn array_value_returns_zero() {
            assert_eq!(
                ObsDriver::encoder_value_to_delta(&json!([1, 2, 3]), 2.0),
                0.0
            );
        }

        #[test]
        fn object_value_returns_zero() {
            assert_eq!(
                ObsDriver::encoder_value_to_delta(&json!({"key": "value"}), 2.0),
                0.0
            );
        }

        #[test]
        fn respects_custom_step_value() {
            // Clockwise with step=10
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(10.0), 10.0), 10.0);
            // Counter-clockwise with step=0.5
            assert_eq!(ObsDriver::encoder_value_to_delta(&json!(100.0), 0.5), -0.5);
        }
    }
}
