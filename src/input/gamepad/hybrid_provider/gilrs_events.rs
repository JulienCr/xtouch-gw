//! gilrs event polling and conversion with radial normalization

use gilrs::{Event, EventType};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::HybridProviderState;
use crate::config::AnalogConfig;
use crate::input::gamepad::axis::gilrs_axis_to_control_id;
use crate::input::gamepad::hybrid_id::HybridControllerId;
use crate::input::gamepad::normalize::normalize_gilrs_stick;
use crate::input::gamepad::provider::GamepadEvent;
use crate::input::gamepad::stick_buffer::{StickBuffer, StickId};

impl HybridProviderState {
    /// Poll gilrs events (non-blocking)
    pub(super) fn poll_gilrs_events(&mut self, event_tx: &mpsc::UnboundedSender<GamepadEvent>) {
        while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
            let hybrid_id = HybridControllerId::from_gilrs(id);

            let (prefix, analog_config) = if let Some(ref manager) = self.slot_manager {
                if let Some(slot) = manager.get_slot_by_id(hybrid_id) {
                    (slot.control_id_prefix(), slot.analog_config.clone())
                } else {
                    continue;
                }
            } else {
                ("gamepad".to_string(), None)
            };

            for gamepad_event in self.convert_gilrs_event(id, event, &prefix, analog_config) {
                debug!("gilrs event: {:?}", gamepad_event);

                if event_tx.send(gamepad_event).is_err() {
                    warn!("Event receiver dropped, shutting down gamepad loop");
                    return;
                }
            }
        }
    }

    /// Convert gilrs event to GamepadEvent(s) with radial normalization for sticks
    fn convert_gilrs_event(
        &mut self,
        id: gilrs::GamepadId,
        event: EventType,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Vec<GamepadEvent> {
        use gilrs::Axis;

        match event {
            EventType::ButtonPressed(button, _) | EventType::ButtonReleased(button, _) => {
                let pressed = matches!(event, EventType::ButtonPressed(_, _));
                match crate::input::gamepad::buttons::gilrs_button_to_control_id(button, prefix) {
                    Some(control_id) => vec![GamepadEvent::Button {
                        control_id,
                        pressed,
                    }],
                    None => vec![],
                }
            },
            EventType::AxisChanged(axis, value, _) => {
                let stick_id = match axis {
                    Axis::LeftStickX | Axis::LeftStickY => Some(StickId::Left),
                    Axis::RightStickX | Axis::RightStickY => Some(StickId::Right),
                    _ => None,
                };

                if let Some(stick) = stick_id {
                    self.process_stick_axis(id, axis, value, stick, prefix, analog_config)
                } else {
                    self.process_non_stick_axis(id, axis, value, prefix, analog_config)
                }
            },
            EventType::Connected => {
                trace!("gilrs gamepad connected event");
                vec![]
            },
            EventType::Disconnected => {
                debug!("gilrs gamepad disconnected event");
                self.last_gilrs_axis_values
                    .retain(|(gp_id, _), _| *gp_id != id);
                self.gilrs_stick_buffer.retain(|(gp_id, _), _| *gp_id != id);
                vec![]
            },
            _ => vec![],
        }
    }

    /// Process stick axis with radial normalization
    fn process_stick_axis(
        &mut self,
        id: gilrs::GamepadId,
        axis: gilrs::Axis,
        value: f32,
        stick: StickId,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Vec<GamepadEvent> {
        use gilrs::Axis;

        let buffer_key = (id, stick);
        let buffer = self
            .gilrs_stick_buffer
            .entry(buffer_key)
            .or_insert_with(StickBuffer::default);

        match axis {
            Axis::LeftStickX | Axis::RightStickX => buffer.x = value,
            Axis::LeftStickY | Axis::RightStickY => buffer.y = value,
            _ => unreachable!(),
        }

        let (norm_x, norm_y) = normalize_gilrs_stick(buffer.x, buffer.y);
        let final_y = -norm_y; // Invert Y to match HID convention

        let (x_axis, y_axis) = match stick {
            StickId::Left => (Axis::LeftStickX, Axis::LeftStickY),
            StickId::Right => (Axis::RightStickX, Axis::RightStickY),
        };

        let mut events = Vec::new();

        if let Some(event) =
            self.emit_axis_with_zero_detection(id, x_axis, norm_x, prefix, analog_config.clone())
        {
            events.push(event);
        }

        if let Some(event) =
            self.emit_axis_with_zero_detection(id, y_axis, final_y, prefix, analog_config)
        {
            events.push(event);
        }

        events
    }

    /// Emit an axis event with zero-crossing detection
    fn emit_axis_with_zero_detection(
        &mut self,
        id: gilrs::GamepadId,
        axis: gilrs::Axis,
        value: f32,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Option<GamepadEvent> {
        const ZERO_THRESHOLD: f32 = 0.05;
        const CHANGE_THRESHOLD: f32 = 0.001;

        let key = (id, axis);
        let last_value = self.last_gilrs_axis_values.get(&key).copied();
        let is_near_zero = value.abs() < ZERO_THRESHOLD;
        let was_nonzero = last_value.is_some_and(|v| v.abs() >= ZERO_THRESHOLD);

        if is_near_zero {
            if was_nonzero {
                self.last_gilrs_axis_values.remove(&key);
                self.axis_sequence += 1;
                return Some(GamepadEvent::Axis {
                    control_id: gilrs_axis_to_control_id(axis, prefix),
                    value: 0.0,
                    analog_config,
                    sequence: self.axis_sequence,
                });
            }
            None
        } else {
            let should_emit =
                last_value.is_none() || (value - last_value.unwrap()).abs() > CHANGE_THRESHOLD;
            if should_emit {
                self.last_gilrs_axis_values.insert(key, value);
                self.axis_sequence += 1;
                Some(GamepadEvent::Axis {
                    control_id: gilrs_axis_to_control_id(axis, prefix),
                    value,
                    analog_config,
                    sequence: self.axis_sequence,
                })
            } else {
                None
            }
        }
    }

    /// Process non-stick axis (triggers, etc.) without radial normalization
    fn process_non_stick_axis(
        &mut self,
        id: gilrs::GamepadId,
        axis: gilrs::Axis,
        value: f32,
        prefix: &str,
        analog_config: Option<AnalogConfig>,
    ) -> Vec<GamepadEvent> {
        match self.emit_axis_with_zero_detection(id, axis, value, prefix, analog_config) {
            Some(event) => vec![event],
            None => vec![],
        }
    }
}
