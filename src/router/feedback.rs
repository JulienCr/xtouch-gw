//! Application feedback processing and transformation

use crate::control_mapping::{load_default_mappings, MidiSpec};
use crate::state::{build_entry_from_raw, AppKey, MidiStateEntry};
use tracing::{debug, trace, warn};

impl super::Router {
    /// Process feedback from an application (reverse transformation)
    pub async fn process_feedback(&self, app_name: &str, raw_data: &[u8]) -> Option<Vec<u8>> {
        // Parse incoming MIDI from app
        let input_msg = match crate::midi::MidiMessage::parse(raw_data) {
            Some(msg) => msg,
            None => return Some(raw_data.to_vec()), // Pass through invalid/sys messages
        };

        // Get normalized value from input
        let normalized_value = match input_msg {
            crate::midi::MidiMessage::PitchBend { value, .. } => {
                crate::midi::convert::to_percent_14bit(value) / 100.0
            },
            crate::midi::MidiMessage::ControlChange { value, .. } => {
                crate::midi::convert::to_percent_7bit(value) / 100.0
            },
            crate::midi::MidiMessage::NoteOn { velocity, .. } => {
                crate::midi::convert::to_percent_7bit(velocity) / 100.0
            },
            crate::midi::MidiMessage::NoteOff { .. } => 0.0,
            _ => return Some(raw_data.to_vec()), // Pass through other messages
        };

        // PAGE-AWARE FILTERING: Check if app is mapped on active page BEFORE scheduling setpoints
        // This prevents faders from moving on Page 2 when Voicemeeter sends feedback
        let config = self.config.read().await;
        let active_page_idx = *self.active_page_index.read().await;

        let active_page = match config.pages.get(active_page_idx) {
            Some(page) => page,
            None => {
                trace!("No active page, skipping feedback forward to X-Touch");
                return None;
            }
        };

        let apps_on_page = self.get_apps_for_page(active_page, &config);
        if !apps_on_page.contains(app_name) {
            trace!(
                "App '{}' not mapped on active page '{}', skipping X-Touch forward",
                app_name,
                active_page.name
            );
            return None;
        }

        debug!(
            "✓ App '{}' is mapped on page '{}', forwarding feedback to X-Touch",
            app_name,
            active_page.name
        );

        // CRITICAL: Schedule motor setpoints AFTER page filtering
        // Only schedule if the app is actually on this page (prevents off-page movements)
        if let crate::midi::MidiMessage::PitchBend { channel, value } = input_msg {
            let channel1 = channel + 1; // Convert 0-based to 1-based
            debug!(
                "← Scheduling fader setpoint from {}: ch={} value14={}",
                app_name, channel1, value
            );
            self.fader_setpoint.schedule(channel1, value, None);
        }

        // Helper to check if a mapping matches the incoming message
        let matches_mapping = |mapping: &crate::config::ControlMapping| -> bool {
            if mapping.app != app_name {
                return false;
            }

            if let Some(midi_spec) = &mapping.midi {
                match midi_spec.midi_type {
                    crate::config::MidiType::Cc => {
                        if let (Some(target_ch), Some(target_cc)) =
                            (midi_spec.channel, midi_spec.cc)
                        {
                            if let crate::midi::MidiMessage::ControlChange { channel, cc, .. } =
                                input_msg
                            {
                                return channel == target_ch.saturating_sub(1) && cc == target_cc;
                            }
                        }
                    },
                    crate::config::MidiType::Note => {
                        if let (Some(target_ch), Some(target_note)) =
                            (midi_spec.channel, midi_spec.note)
                        {
                            match input_msg {
                                crate::midi::MidiMessage::NoteOn { channel, note, .. }
                                | crate::midi::MidiMessage::NoteOff { channel, note, .. } => {
                                    return channel == target_ch.saturating_sub(1)
                                        && note == target_note;
                                },
                                _ => {},
                            }
                        }
                    },
                    crate::config::MidiType::Pb => {
                        if let Some(target_ch) = midi_spec.channel {
                            if let crate::midi::MidiMessage::PitchBend { channel, .. } = input_msg {
                                return channel == target_ch.saturating_sub(1);
                            }
                        }
                    },
                    _ => {},
                }
            }
            false
        };

        // Search in active page controls (use the active_page we already have)
        let mut found_control_id = None;

        if let Some(controls) = &active_page.controls {
            for (id, mapping) in controls {
                if matches_mapping(mapping) {
                    found_control_id = Some(id.clone());
                    break;
                }
            }
        }

        // If not found, search in global controls
        if found_control_id.is_none() {
            if let Some(global) = &config.pages_global {
                if let Some(controls) = &global.controls {
                    for (id, mapping) in controls {
                        if matches_mapping(mapping) {
                            found_control_id = Some(id.clone());
                            break;
                        }
                    }
                }
            }
        }

        // Also check X-Touch mode
        let is_mcu_mode = config
            .xtouch
            .as_ref()
            .map(|x| matches!(x.mode, crate::config::XTouchMode::Mcu))
            .unwrap_or(true);

        drop(config);

        if let Some(control_id) = found_control_id {
            // Load hardware mapping to find native message
            if let Ok(db) = load_default_mappings() {
                if let Some(native_spec) = db.get_midi_spec(&control_id, is_mcu_mode) {
                    // Construct native message with scaled value
                    let native_msg = match native_spec {
                        MidiSpec::ControlChange { cc } => {
                            // For X-Touch, CCs are usually on channel 1 (0)
                            // But we should probably check the group or assume standard MCU
                            Some(crate::midi::MidiMessage::ControlChange {
                                channel: 0, // Default to Ch 1 for buttons
                                cc,
                                value: crate::midi::convert::from_percent_7bit(
                                    normalized_value * 100.0,
                                ),
                            })
                        },
                        MidiSpec::Note { note } => {
                            let velocity =
                                crate::midi::convert::from_percent_7bit(normalized_value * 100.0);
                            if velocity == 0 {
                                Some(crate::midi::MidiMessage::NoteOff {
                                    channel: 0, // Default to Ch 1 for buttons
                                    note,
                                    velocity: 0,
                                })
                            } else {
                                Some(crate::midi::MidiMessage::NoteOn {
                                    channel: 0, // Default to Ch 1 for buttons
                                    note,
                                    velocity,
                                })
                            }
                        },
                        MidiSpec::PitchBend { channel } => {
                            let value14 =
                                crate::midi::convert::from_percent_14bit(normalized_value * 100.0);

                            // CRITICAL: Schedule motor setpoint for CC→PB transformations
                            // This handles the case where QLC+ sends CC but the fader needs PB
                            let channel1 = channel + 1; // Convert 0-based to 1-based
                            debug!(
                                "← Scheduling fader setpoint (CC→PB): {} -> ch={} value14={}",
                                app_name, channel1, value14
                            );
                            self.fader_setpoint.schedule(channel1, value14, None);

                            Some(crate::midi::MidiMessage::PitchBend {
                                channel,
                                value: value14,
                            })
                        },
                    };

                    if let Some(msg) = native_msg {
                        debug!(
                            "← Feedback Transform: {} -> {} ({} -> {})",
                            app_name, control_id, input_msg, msg
                        );
                        return Some(msg.to_bytes());
                    }
                }
            }
        }

        // No mapping found, pass through raw
        Some(raw_data.to_vec())
    }

    /// Ingest MIDI feedback from an application
    ///
    /// This is the entry point for feedback from applications (OBS, Voicemeeter, etc.).
    /// It updates the state store and forwards to X-Touch if relevant for the active page.
    ///
    /// # Arguments
    ///
    /// * `app_key` - Application identifier (e.g., "obs", "voicemeeter")
    /// * `raw` - Raw MIDI bytes from the application
    /// * `port_id` - MIDI port identifier
    pub fn on_midi_from_app(&self, app_key: &str, raw: &[u8], port_id: &str) {
        // Parse the MIDI message
        let entry = match build_entry_from_raw(raw, port_id) {
            Some(e) => e,
            None => {
                debug!("Failed to parse MIDI from app '{}': {:02X?}", app_key, raw);
                return;
            },
        };

        // Update state store (this also notifies subscribers)
        let app = match AppKey::from_str(app_key) {
            Some(a) => a,
            None => {
                warn!("Unknown application key: {}", app_key);
                return;
            },
        };

        // Update state store
        self.state.update_from_feedback(app, entry.clone());

        // Log for debugging
        trace!(
            "State <- {}: status={:?} ch={:?} d1={:?} val={:?}",
            app_key,
            entry.addr.status,
            entry.addr.channel,
            entry.addr.data1,
            entry.value
        );

        // Mark this as sent to app (for anti-echo)
        self.update_app_shadow(app_key, &entry);

        // TODO: Forward to X-Touch if relevant for active page
        // This will be implemented in the forward module (Phase 6.2)
    }
}

