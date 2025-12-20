//! MIDI configuration section
//!
//! Handles MIDI port configuration and per-app MIDI routing

use crate::config::MidiAppConfig;
use crate::config_editor::{state::EditorState, validation};
use super::common;

/// Render MIDI configuration section
pub fn render(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.collapsing("MIDI Configuration", |ui| {
        ui.add_space(5.0);

        // Input port
        let input_port_error = state.get_error("midi.input_port");
        let (changed, error) = common::validated_text_edit(
            ui,
            "Input Port:",
            &mut state.config.midi.input_port,
            input_port_error.as_ref(),
            validation::validate_midi_port,
        );
        if changed {
            state.mark_dirty();
            if let Some(err) = error {
                state.set_error("midi.input_port", err);
            } else {
                state.clear_error("midi.input_port");
            }
        }

        ui.add_space(5.0);

        // Output port
        let output_port_error = state.get_error("midi.output_port");
        let (changed, error) = common::validated_text_edit(
            ui,
            "Output Port:",
            &mut state.config.midi.output_port,
            output_port_error.as_ref(),
            validation::validate_midi_port,
        );
        if changed {
            state.mark_dirty();
            if let Some(err) = error {
                state.set_error("midi.output_port", err);
            } else {
                state.clear_error("midi.output_port");
            }
        }

        ui.add_space(10.0);

        // MIDI Apps section
        ui.label(egui::RichText::new("MIDI Apps:").strong());

        // Ensure apps vec exists
        if state.config.midi.apps.is_none() {
            state.config.midi.apps = Some(Vec::new());
        }

        if state.config.midi.apps.is_some() {
            let mut apps_to_remove = Vec::new();
            let app_count = state.config.midi.apps.as_ref().unwrap().len();

            for idx in 0..app_count {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        // App name - get error and value first
                        let field_path = format!("midi.apps.{}.name", idx);
                        let app_name_error = state.get_error(&field_path);
                        let mut app_name = state.config.midi.apps.as_ref().unwrap()[idx].name.clone();

                        let (changed, error) = common::validated_text_edit(
                            ui,
                            "Name:",
                            &mut app_name,
                            app_name_error.as_ref(),
                            validation::validate_app_name,
                        );
                        if changed {
                            state.config.midi.apps.as_mut().unwrap()[idx].name = app_name;
                            state.mark_dirty();
                            if let Some(err) = error {
                                state.set_error(&field_path, err);
                            } else {
                                state.clear_error(&field_path);
                            }
                        }

                        // Remove button
                        if ui.button("✖").on_hover_text("Remove app").clicked() {
                            apps_to_remove.push(idx);
                            state.mark_dirty();
                        }
                    });

                    ui.horizontal(|ui| {
                        // Output port (optional)
                        let mut output_port = state.config.midi.apps.as_ref().unwrap()[idx]
                            .output_port.clone().unwrap_or_default();
                        ui.label("Output Port:");
                        if ui.text_edit_singleline(&mut output_port).changed() {
                            state.config.midi.apps.as_mut().unwrap()[idx].output_port =
                                if output_port.is_empty() {
                                    None
                                } else {
                                    Some(output_port)
                                };
                            state.mark_dirty();
                        }
                    });

                    ui.horizontal(|ui| {
                        // Input port (optional)
                        let mut input_port = state.config.midi.apps.as_ref().unwrap()[idx]
                            .input_port.clone().unwrap_or_default();
                        ui.label("Input Port:");
                        if ui.text_edit_singleline(&mut input_port).changed() {
                            state.config.midi.apps.as_mut().unwrap()[idx].input_port =
                                if input_port.is_empty() {
                                    None
                                } else {
                                    Some(input_port)
                                };
                            state.mark_dirty();
                        }
                    });
                });

                ui.add_space(5.0);
            }

            // Remove apps marked for deletion (in reverse order to preserve indices)
            if let Some(apps) = &mut state.config.midi.apps {
                for idx in apps_to_remove.iter().rev() {
                    apps.remove(*idx);
                }
            }

            ui.add_space(5.0);

            // Add app button
            if ui.button("➕ Add MIDI App").clicked() {
                if let Some(apps) = &mut state.config.midi.apps {
                    apps.push(MidiAppConfig {
                        name: String::new(),
                        output_port: None,
                        input_port: None,
                    });
                    state.mark_dirty();
                }
            }
        }
    });
}
