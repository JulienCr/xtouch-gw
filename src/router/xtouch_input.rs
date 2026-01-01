//! X-Touch MIDI input handling and routing

use serde_json::Value;
use tracing::{debug, trace, warn};

impl super::Router {
    /// Process MIDI input from X-Touch hardware
    ///
    /// Handles:
    /// - Page navigation (F1-F8, prev/next buttons)
    /// - Control routing (faders, buttons, encoders → drivers)
    pub async fn on_midi_from_xtouch(&self, raw: &[u8]) {
        use crate::control_mapping::{load_default_mappings, MidiSpec};

        if raw.len() < 2 {
            return;
        }

        // Record activity for tray UI
        if let Some(ref tracker) = self.activity_tracker {
            tracker.record("xtouch", crate::tray::ActivityDirection::Inbound);
        }

        let status = raw[0];
        let type_nibble = (status & 0xF0) >> 4;
        let channel = (status & 0x0F) + 1;

        // First, check for page navigation (Note On messages only)
        if type_nibble == 0x9 && raw.len() >= 3 {
            let note = raw[1];
            let velocity = raw[2];

            // Ignore Note Off (velocity 0)
            if velocity != 0 {
                // Get paging configuration
                let config = self.config.read().await;
                let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1);
                let prev_note = config.paging.as_ref().map(|p| p.prev_note).unwrap_or(46);
                let next_note = config.paging.as_ref().map(|p| p.next_note).unwrap_or(47);
                drop(config);

                // Only process navigation on the paging channel
                if channel == paging_channel {
                    // Check for prev/next navigation
                    if note == prev_note {
                        debug!("X-Touch: Previous page (note {})", note);
                        self.prev_page().await;
                        return;
                    }

                    if note == next_note {
                        debug!("X-Touch: Next page (note {})", note);
                        self.next_page().await;
                        return;
                    }

                    // Check for F-key direct page access (F1-F8 = notes 54-61)
                    if (54..=61).contains(&note) {
                        let page_index = (note - 54) as usize;
                        debug!(
                            "X-Touch: Direct page access F{} (note {})",
                            page_index + 1,
                            note
                        );

                        let config = self.config.read().await;
                        if page_index < config.pages.len() {
                            drop(config);
                            let _ = self.set_active_page(&page_index.to_string()).await;
                        }
                        return;
                    }
                }
            }
        }

        // Route control events to drivers
        let config = self.config.read().await;
        let is_mcu_mode = config
            .xtouch
            .as_ref()
            .map(|x| matches!(x.mode, crate::config::XTouchMode::Mcu))
            .unwrap_or(true); // Default to MCU mode
        drop(config);

        // Load control mappings
        let mapping_db = match load_default_mappings() {
            Ok(db) => db,
            Err(e) => {
                warn!("Failed to load control mappings: {}", e);
                return;
            },
        };

        // Parse incoming MIDI to determine which control was triggered
        let midi_spec = match MidiSpec::from_raw(raw) {
            Ok(spec) => spec,
            Err(_) => {
                trace!("Unsupported MIDI message: {:02X?}", raw);
                return;
            },
        };

        // Find the control ID from MIDI message
        let control_id = match mapping_db.find_control_by_midi(&midi_spec, is_mcu_mode) {
            Some(id) => id,
            None => {
                trace!("No control mapping found for MIDI: {:?}", midi_spec);
                return;
            },
        };

        debug!(
            "X-Touch control triggered: {} (MIDI: {:?})",
            control_id, midi_spec
        );

        // Mark user action for Last-Write-Wins
        self.mark_user_action(raw);

        // CRITICAL: Update fader setpoint for user actions (PitchBend from X-Touch)
        // This ensures the motor tracks the user's physical position
        if type_nibble == 0xE && raw.len() >= 3 {
            // PitchBend message - mask to 7 bits for safety
            let lsb = raw[1] & 0x7F;
            let msb = raw[2] & 0x7F;
            let value14 = ((msb as u16) << 7) | (lsb as u16);
            debug!(
                "← User moved fader: ch={} value14={}",
                channel, value14
            );
            self.fader_setpoint.schedule(channel as u8, value14, None);
        }

        // Get active page and find control configuration
        let page = match self.get_active_page().await {
            Some(p) => p,
            None => {
                warn!("No active page");
                return;
            },
        };

        // Check page-specific controls first, then global controls as fallback
        let config = self.config.read().await;
        let control_config = page.controls
            .as_ref()
            .and_then(|controls| controls.get(control_id))
            .or_else(|| {
                config
                    .pages_global
                    .as_ref()
                    .and_then(|pg| pg.controls.as_ref())
                    .and_then(|controls| controls.get(control_id))
            });

        let control_config = match control_config {
            Some(cc) => cc.clone(),
            None => {
                trace!(
                    "Control '{}' not mapped on page '{}'",
                    control_id,
                    page.name
                );
                return;
            },
        };
        drop(config);

        // Check if this is MIDI direct mode (send raw MIDI to bridge)
        if let Some(target_spec) = &control_config.midi {
            self.handle_midi_direct_mode(raw, control_id, &control_config, target_spec).await;
            return;
        }

        // Driver action mode
        self.handle_driver_action_mode(raw, control_id, &control_config).await;
    }

    /// Handle MIDI direct mode (transform and send to bridge)
    async fn handle_midi_direct_mode(
        &self,
        raw: &[u8],
        control_id: &str,
        control_config: &crate::config::ControlMapping,
        target_spec: &crate::config::MidiSpec,
    ) {
        // 1. Parse input MIDI to get normalized value (0.0 - 1.0)
        let input_msg = crate::midi::MidiMessage::parse(raw);
        let normalized_value = match input_msg {
            Some(msg) => match msg {
                crate::midi::MidiMessage::PitchBend { value, .. } => {
                    crate::midi::convert::to_percent_14bit(value) / 100.0
                },
                crate::midi::MidiMessage::ControlChange { value, .. } => {
                    crate::midi::convert::to_percent_7bit(value) / 100.0
                },
                crate::midi::MidiMessage::NoteOn { velocity, .. } => {
                    crate::midi::convert::to_percent_7bit(velocity) / 100.0
                },
                _ => 0.0,
            },
            None => 0.0,
        };

        // 2. Construct target message based on config
        let target_msg = match target_spec.midi_type {
            crate::config::MidiType::Cc => {
                if let (Some(ch), Some(cc)) = (target_spec.channel, target_spec.cc) {
                    let value =
                        crate::midi::convert::from_percent_7bit(normalized_value * 100.0);
                    Some(crate::midi::MidiMessage::ControlChange {
                        channel: ch.saturating_sub(1), // Config is 1-based, internal is 0-based
                        cc,
                        value,
                    })
                } else {
                    None
                }
            },
            crate::config::MidiType::Note => {
                if let (Some(ch), Some(note)) = (target_spec.channel, target_spec.note) {
                    let velocity =
                        crate::midi::convert::from_percent_7bit(normalized_value * 100.0);
                    // If velocity is 0, send NoteOff, otherwise NoteOn
                    if velocity == 0 {
                        Some(crate::midi::MidiMessage::NoteOff {
                            channel: ch.saturating_sub(1),
                            note,
                            velocity: 0,
                        })
                    } else {
                        Some(crate::midi::MidiMessage::NoteOn {
                            channel: ch.saturating_sub(1),
                            note,
                            velocity,
                        })
                    }
                } else {
                    None
                }
            },
            crate::config::MidiType::Pb => {
                if let Some(ch) = target_spec.channel {
                    let value =
                        crate::midi::convert::from_percent_14bit(normalized_value * 100.0);
                    Some(crate::midi::MidiMessage::PitchBend {
                        channel: ch.saturating_sub(1),
                        value,
                    })
                } else {
                    None
                }
            },
            crate::config::MidiType::Passthrough => {
                // Raw passthrough (no transformation)
                crate::midi::MidiMessage::parse(raw)
            },
        };

        if let Some(msg) = target_msg {
            let bytes = msg.to_bytes();
            debug!(
                "→ Transform: {} -> {} ({} bytes) to '{}'",
                control_id,
                msg,
                bytes.len(),
                control_config.app
            );

            // Get the MIDI bridge driver for this app
            let driver = {
                let drivers = self.drivers.read().await;
                drivers.get(&control_config.app).cloned()
            };

            if let Some(driver) = driver {
                // Create execution context with transformed MIDI
                let mut ctx = self.create_execution_context().await;
                ctx.value = Some(match serde_json::to_value(&bytes) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Failed to serialize MIDI data: {}", e);
                        return;
                    },
                });

                // Call passthrough action on the bridge
                if let Err(e) = driver.execute("passthrough", vec![], ctx).await {
                    warn!("Failed to passthrough MIDI: {}", e);
                }
            } else {
                warn!(
                    "Bridge driver '{}' not found for MIDI passthrough",
                    control_config.app
                );
            }
        }
    }

    /// Handle driver action mode (execute action on driver)
    async fn handle_driver_action_mode(
        &self,
        raw: &[u8],
        control_id: &str,
        control_config: &crate::config::ControlMapping,
    ) {
        let driver = {
            let drivers = self.drivers.read().await;
            drivers.get(&control_config.app).cloned()
        };

        let driver = match driver {
            Some(d) => d,
            None => {
                warn!(
                    "Driver '{}' not found for control '{}'",
                    control_config.app, control_id
                );
                return;
            },
        };

        // Determine the action to execute
        let action = control_config.action.as_deref().unwrap_or("execute");

        // Build parameters
        let params = control_config.params.clone().unwrap_or_default();

        // Filter button releases (Note On with velocity 0)
        // This prevents toggle actions from firing twice (press + release)
        let status = raw[0];
        let type_nibble = (status & 0xF0) >> 4;
        if type_nibble == 0x9 && raw.len() >= 3 {
            let velocity = raw[2];
            if velocity == 0 {
                debug!("Ignoring Note Off (velocity 0) for control '{}'", control_id);
                return;
            }
        }

        // Create execution context with parsed MIDI value
        let mut ctx = self.create_execution_context().await;

        // Set control ID so drivers can detect input source (gamepad vs encoder)
        ctx.control_id = Some(control_id.to_string());

        // Parse MIDI message to extract the value/velocity byte
        // This allows drivers to receive a Number instead of raw bytes array
        if raw.len() >= 3 {
            let value = match type_nibble {
                0x9 => raw[2] as f64,        // Note On: velocity
                0xB => raw[2] as f64,        // Control Change: value
                0xE => {                      // Pitch Bend: 14-bit value (0-16383)
                    let lsb = raw[1] as u16;
                    let msb = raw[2] as u16;
                    ((msb << 7) | lsb) as f64
                },
                _ => {
                    // Unknown message type, pass raw bytes
                    match serde_json::to_value(raw) {
                        Ok(v) => {
                            ctx.value = Some(v);
                            0.0 // unused
                        },
                        Err(e) => {
                            warn!("Failed to serialize MIDI data: {}", e);
                            return;
                        },
                    }
                },
            };

            if type_nibble == 0x9 || type_nibble == 0xB || type_nibble == 0xE {
                ctx.value = Some(Value::Number(serde_json::Number::from_f64(value).unwrap()));
            }
        } else {
            // Short message, pass raw bytes
            ctx.value = Some(match serde_json::to_value(raw) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Failed to serialize MIDI data: {}", e);
                    return;
                },
            });
        }

        // Execute driver action
        debug!(
            "→ Routing: {} → app={} action={} (value={:?})",
            control_id, control_config.app, action, ctx.value
        );
        if let Err(e) = driver.execute(action, params, ctx).await {
            warn!("Driver execution failed: {}", e);
        }
    }
}

