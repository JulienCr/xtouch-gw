//! Gamepad configuration section

use crate::config::{GamepadConfig, AnalogConfig, GamepadSlotConfig};
use crate::config_editor::state::EditorState;
use super::common;

/// Render gamepad configuration section
pub fn render(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.heading("Gamepad Configuration");
    ui.add_space(10.0);

    // Enable/disable checkbox
    let mut enabled = state.config.gamepad.as_ref().map_or(false, |g| g.enabled);
    if common::checkbox_input(ui, "Enable Gamepad Support", &mut enabled) {
        if enabled && state.config.gamepad.is_none() {
            // Create default gamepad config
            state.config.gamepad = Some(GamepadConfig {
                enabled: true,
                provider: "hid".to_string(),
                analog: Some(AnalogConfig {
                    pan_gain: 15.0,
                    zoom_gain: 3.0,
                    deadzone: 0.02,
                    gamma: 1.5,
                    invert: Default::default(),
                }),
                hid: None,
                gamepads: None,
            });
        } else if !enabled && state.config.gamepad.is_some() {
            state.config.gamepad.as_mut().unwrap().enabled = false;
        } else if enabled && state.config.gamepad.is_some() {
            state.config.gamepad.as_mut().unwrap().enabled = true;
        }
        state.mark_dirty();
    }

    ui.add_space(10.0);

    // Only show settings if gamepad exists
    if state.config.gamepad.is_some() {
        let gamepad_enabled = state.config.gamepad.as_ref().unwrap().enabled;

        if gamepad_enabled {
            // Provider (read-only for now, only "hid" supported)
            ui.horizontal(|ui| {
                ui.label("Provider:");
                ui.label(egui::RichText::new("hid (Hardware Input Device)").italics().weak());
            });

            ui.add_space(10.0);

            // Global Analog Settings
            render_analog_settings(ui, state, None);

            ui.add_space(10.0);

            // Multi-gamepad slots
            render_gamepad_slots(ui, state);
        } else {
            ui.label(egui::RichText::new("Gamepad support is disabled").italics().weak());
        }
    }
}

/// Render analog settings for global or per-slot configuration
fn render_analog_settings(ui: &mut egui::Ui, state: &mut EditorState, slot_index: Option<usize>) {
    let section_label = if slot_index.is_some() {
        "Analog Settings (Slot-Specific)"
    } else {
        "Global Analog Settings"
    };

    ui.collapsing(section_label, |ui| {
        ui.add_space(5.0);

        // Check if analog config exists (don't hold the reference)
        let has_analog = if let Some(idx) = slot_index {
            // Per-slot analog config
            state.config.gamepad.as_ref()
                .and_then(|g| g.gamepads.as_ref())
                .and_then(|slots| slots.get(idx))
                .and_then(|slot| slot.analog.as_ref())
                .is_some()
        } else {
            // Global analog config
            state.config.gamepad.as_ref()
                .and_then(|g| g.analog.as_ref())
                .is_some()
        };

        if !has_analog {
            // Create default analog config if it doesn't exist
            if let Some(idx) = slot_index {
                // Per-slot
                if let Some(slots) = state.config.gamepad.as_mut()
                    .and_then(|g| g.gamepads.as_mut())
                {
                    if let Some(slot) = slots.get_mut(idx) {
                        slot.analog = Some(AnalogConfig {
                            pan_gain: 15.0,
                            zoom_gain: 3.0,
                            deadzone: 0.02,
                            gamma: 1.5,
                            invert: Default::default(),
                        });
                        state.mark_dirty();
                    }
                }
            } else {
                // Global
                if let Some(gamepad) = state.config.gamepad.as_mut() {
                    gamepad.analog = Some(AnalogConfig {
                        pan_gain: 15.0,
                        zoom_gain: 3.0,
                        deadzone: 0.02,
                        gamma: 1.5,
                        invert: Default::default(),
                    });
                    state.mark_dirty();
                }
            }
        }

        // Get current analog values (clone them)
        let (mut pan_gain, mut zoom_gain, mut deadzone, mut gamma, invert_map) = if let Some(idx) = slot_index {
            // Per-slot
            state.config.gamepad.as_ref()
                .and_then(|g| g.gamepads.as_ref())
                .and_then(|slots| slots.get(idx))
                .and_then(|slot| slot.analog.as_ref())
                .map(|a| (a.pan_gain, a.zoom_gain, a.deadzone, a.gamma, a.invert.clone()))
                .unwrap_or((15.0, 3.0, 0.02, 1.5, Default::default()))
        } else {
            // Global
            state.config.gamepad.as_ref()
                .and_then(|g| g.analog.as_ref())
                .map(|a| (a.pan_gain, a.zoom_gain, a.deadzone, a.gamma, a.invert.clone()))
                .unwrap_or((15.0, 3.0, 0.02, 1.5, Default::default()))
        };

        // Now render the settings
        {
            // Pan gain
            let (changed, _) = common::validated_f32_input(
                ui,
                "Pan Gain:",
                &mut pan_gain,
                None,
                0.1,
                100.0,
            );
            if changed {
                if let Some(idx) = slot_index {
                    // Per-slot
                    if let Some(slots) = state.config.gamepad.as_mut()
                        .and_then(|g| g.gamepads.as_mut())
                    {
                        if let Some(slot) = slots.get_mut(idx) {
                            if let Some(a) = slot.analog.as_mut() {
                                a.pan_gain = pan_gain;
                                state.mark_dirty();
                            }
                        }
                    }
                } else {
                    // Global
                    if let Some(a) = state.config.gamepad.as_mut()
                        .and_then(|g| g.analog.as_mut())
                    {
                        a.pan_gain = pan_gain;
                        state.mark_dirty();
                    }
                }
            }

            ui.add_space(5.0);

            // Zoom gain
            let (changed, _) = common::validated_f32_input(
                ui,
                "Zoom Gain:",
                &mut zoom_gain,
                None,
                0.1,
                100.0,
            );
            if changed {
                if let Some(idx) = slot_index {
                    // Per-slot
                    if let Some(slots) = state.config.gamepad.as_mut()
                        .and_then(|g| g.gamepads.as_mut())
                    {
                        if let Some(slot) = slots.get_mut(idx) {
                            if let Some(a) = slot.analog.as_mut() {
                                a.zoom_gain = zoom_gain;
                                state.mark_dirty();
                            }
                        }
                    }
                } else {
                    // Global
                    if let Some(a) = state.config.gamepad.as_mut()
                        .and_then(|g| g.analog.as_mut())
                    {
                        a.zoom_gain = zoom_gain;
                        state.mark_dirty();
                    }
                }
            }

            ui.add_space(5.0);

            // Deadzone
            let (changed, _) = common::validated_f32_input(
                ui,
                "Deadzone:",
                &mut deadzone,
                None,
                0.0,
                0.5,
            );
            if changed {
                if let Some(idx) = slot_index {
                    // Per-slot
                    if let Some(slots) = state.config.gamepad.as_mut()
                        .and_then(|g| g.gamepads.as_mut())
                    {
                        if let Some(slot) = slots.get_mut(idx) {
                            if let Some(a) = slot.analog.as_mut() {
                                a.deadzone = deadzone;
                                state.mark_dirty();
                            }
                        }
                    }
                } else {
                    // Global
                    if let Some(a) = state.config.gamepad.as_mut()
                        .and_then(|g| g.analog.as_mut())
                    {
                        a.deadzone = deadzone;
                        state.mark_dirty();
                    }
                }
            }

            ui.add_space(5.0);

            // Gamma
            let (changed, _) = common::validated_f32_input(
                ui,
                "Gamma:",
                &mut gamma,
                None,
                0.1,
                5.0,
            );
            if changed {
                if let Some(idx) = slot_index {
                    // Per-slot
                    if let Some(slots) = state.config.gamepad.as_mut()
                        .and_then(|g| g.gamepads.as_mut())
                    {
                        if let Some(slot) = slots.get_mut(idx) {
                            if let Some(a) = slot.analog.as_mut() {
                                a.gamma = gamma;
                                state.mark_dirty();
                            }
                        }
                    }
                } else {
                    // Global
                    if let Some(a) = state.config.gamepad.as_mut()
                        .and_then(|g| g.analog.as_mut())
                    {
                        a.gamma = gamma;
                        state.mark_dirty();
                    }
                }
            }

            ui.add_space(10.0);

            // Invert axes
            ui.label(egui::RichText::new("Invert Axes:").strong());

            let axes = ["lx", "ly", "rx", "ry", "zl", "zr"];
            for axis_name in axes {
                let mut inverted = invert_map.get(axis_name).copied().unwrap_or(false);
                if common::checkbox_input(ui, &format!("Invert {}", axis_name.to_uppercase()), &mut inverted) {
                    if let Some(idx) = slot_index {
                        // Per-slot
                        if let Some(slots) = state.config.gamepad.as_mut()
                            .and_then(|g| g.gamepads.as_mut())
                        {
                            if let Some(slot) = slots.get_mut(idx) {
                                if let Some(a) = slot.analog.as_mut() {
                                    a.invert.insert(axis_name.to_string(), inverted);
                                    state.mark_dirty();
                                }
                            }
                        }
                    } else {
                        // Global
                        if let Some(a) = state.config.gamepad.as_mut()
                            .and_then(|g| g.analog.as_mut())
                        {
                            a.invert.insert(axis_name.to_string(), inverted);
                            state.mark_dirty();
                        }
                    }
                }
            }

            ui.add_space(5.0);

            ui.label(
                egui::RichText::new("ℹ Default: pan_gain=15.0, zoom_gain=3.0, deadzone=0.02, gamma=1.5")
                    .italics()
                    .weak()
                    .small()
            );
        }
    });
}

/// Render multi-gamepad slots section
fn render_gamepad_slots(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.collapsing("Multi-Gamepad Slots", |ui| {
        ui.add_space(5.0);

        ui.label(
            egui::RichText::new("Configure multiple gamepads with different settings")
                .italics()
                .weak()
        );

        ui.add_space(10.0);

        // Ensure gamepads vec exists
        if state.config.gamepad.as_ref().and_then(|g| g.gamepads.as_ref()).is_none() {
            if let Some(gamepad) = state.config.gamepad.as_mut() {
                gamepad.gamepads = Some(Vec::new());
            }
        }

        let slot_count = state.config.gamepad.as_ref()
            .and_then(|g| g.gamepads.as_ref())
            .map_or(0, |slots| slots.len());

        let mut slots_to_remove = Vec::new();

        for idx in 0..slot_count {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Slot {}", idx + 1)).strong());

                    // Remove button
                    if ui.button("✖").on_hover_text("Remove slot").clicked() {
                        slots_to_remove.push(idx);
                        state.mark_dirty();
                    }
                });

                ui.add_space(5.0);

                // Product match pattern
                let mut product_match = state.config.gamepad.as_ref()
                    .and_then(|g| g.gamepads.as_ref())
                    .and_then(|slots| slots.get(idx))
                    .map_or(String::new(), |slot| slot.product_match.clone());

                ui.horizontal(|ui| {
                    ui.label("Product Match:");
                    if ui.text_edit_singleline(&mut product_match).changed() {
                        if let Some(slots) = state.config.gamepad.as_mut()
                            .and_then(|g| g.gamepads.as_mut())
                        {
                            if let Some(slot) = slots.get_mut(idx) {
                                slot.product_match = product_match;
                                state.mark_dirty();
                            }
                        }
                    }
                });

                ui.add_space(5.0);

                // Per-slot analog settings
                render_analog_settings(ui, state, Some(idx));
            });

            ui.add_space(10.0);
        }

        // Remove slots marked for deletion (in reverse order)
        if let Some(slots) = state.config.gamepad.as_mut()
            .and_then(|g| g.gamepads.as_mut())
        {
            for idx in slots_to_remove.iter().rev() {
                slots.remove(*idx);
            }
        }

        // Add slot button
        if ui.button("➕ Add Gamepad Slot").clicked() {
            if let Some(slots) = state.config.gamepad.as_mut()
                .and_then(|g| g.gamepads.as_mut())
            {
                slots.push(GamepadSlotConfig {
                    product_match: String::new(),
                    analog: None,
                });
                state.mark_dirty();
            }
        }

        ui.add_space(5.0);

        ui.label(
            egui::RichText::new("ℹ Product match is case-insensitive substring (e.g., \"Xbox\" matches \"Xbox Series X Controller\")")
                .italics()
                .weak()
                .small()
        );
    });
}
