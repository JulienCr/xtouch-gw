//! XInput controller polling and event generation

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::HybridProviderState;
use crate::input::gamepad::hybrid_id::HybridControllerId;
use crate::input::gamepad::provider::GamepadEvent;
use crate::input::gamepad::xinput_convert::{
    convert_xinput_axes, convert_xinput_buttons, poll_xinput_controller, CachedXInputState,
};

impl HybridProviderState {
    /// Poll XInput events for all 4 possible controllers
    pub(super) fn poll_xinput_events(&mut self, event_tx: &mpsc::Sender<GamepadEvent>) {
        if self.xinput_handle.is_none() {
            return;
        }

        for user_index in 0..4u32 {
            let idx = user_index as usize;

            let state = {
                let handle = self.xinput_handle.as_ref().unwrap();
                match poll_xinput_controller(handle, user_index) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        if self.xinput_connected[idx] {
                            self.xinput_connected[idx] = false;
                            self.last_xinput_state[idx] = None;
                        }
                        continue;
                    },
                    Err(e) => {
                        warn!("XInput error for user {}: {:?}", user_index, e);
                        continue;
                    },
                }
            };

            // Check packet number for changes
            if let Some(last_state) = &self.last_xinput_state[idx] {
                if last_state.packet_number == state.raw.dwPacketNumber {
                    continue;
                }
            }

            self.handle_xinput_update(idx, state, event_tx);
        }
    }

    /// Handle XInput controller update (generate button and axis events)
    fn handle_xinput_update(
        &mut self,
        user_index: usize,
        state: rusty_xinput::XInputState,
        event_tx: &mpsc::Sender<GamepadEvent>,
    ) {
        let hybrid_id = HybridControllerId::from_xinput(user_index);

        let (prefix, analog_config) = if let Some(ref mut manager) = self.slot_manager {
            if let Some(slot) = manager.get_slot_by_id(hybrid_id) {
                (slot.control_id_prefix(), slot.analog_config.clone())
            } else {
                let name = format!("XInput Controller {}", user_index + 1);
                if manager.try_connect(hybrid_id, &name).is_some() {
                    self.xinput_connected[user_index] = true;
                    if let Some(slot) = manager.get_slot_by_id(hybrid_id) {
                        (slot.control_id_prefix(), slot.analog_config.clone())
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            }
        } else {
            ("gamepad".to_string(), None)
        };

        // Generate button events
        let old_buttons = self.last_xinput_state[user_index]
            .as_ref()
            .map(|s| s.buttons);
        let new_buttons = state.raw.Gamepad.wButtons;
        let button_events = convert_xinput_buttons(old_buttons, new_buttons, &prefix);

        for event in button_events {
            debug!("XInput button event: {:?}", event);
            if event_tx.try_send(event).is_err() {
                warn!("Event receiver dropped, shutting down XInput polling");
                return;
            }
        }

        // Generate axis events
        let axis_events = convert_xinput_axes(
            self.last_xinput_state[user_index].as_ref(),
            &state,
            &prefix,
            analog_config,
            &mut self.axis_sequence,
        );

        for event in axis_events {
            debug!("XInput axis event: {:?}", event);
            if event_tx.try_send(event).is_err() {
                warn!("Event receiver dropped, shutting down XInput polling");
                return;
            }
        }

        // Update cached state
        self.last_xinput_state[user_index] = Some(CachedXInputState::from(&state));
        self.xinput_connected[user_index] = true;
    }
}
