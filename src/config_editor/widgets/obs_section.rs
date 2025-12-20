//! OBS WebSocket configuration section

use crate::config::ObsConfig;
use crate::config_editor::{state::EditorState, validation};
use super::common;

/// Render OBS configuration section
pub fn render(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.collapsing("OBS Configuration", |ui| {
        ui.add_space(5.0);

        // Enable/disable checkbox
        let mut enabled = state.config.obs.is_some();
        if common::checkbox_input(ui, "Enable OBS Integration", &mut enabled) {
            if enabled && state.config.obs.is_none() {
                // Create default OBS config
                state.config.obs = Some(ObsConfig {
                    host: "localhost".to_string(),
                    port: 4455,
                    password: None,
                });
            } else if !enabled {
                state.config.obs = None;
            }
            state.mark_dirty();
        }

        ui.add_space(5.0);

        // Only show settings if enabled
        if state.config.obs.is_some() {
            // Host - get error first, then borrow config
            let host_error = state.get_error("obs.host");
            let mut host = state.config.obs.as_ref().unwrap().host.clone();
            let (changed, error) = common::validated_text_edit(
                ui,
                "Host:",
                &mut host,
                host_error.as_ref(),
                validation::validate_hostname,
            );
            if changed {
                state.config.obs.as_mut().unwrap().host = host;
                state.mark_dirty();
                if let Some(err) = error {
                    state.set_error("obs.host", err);
                } else {
                    state.clear_error("obs.host");
                }
            }

            ui.add_space(5.0);

            // Port
            let port_error = state.get_error("obs.port");
            let mut port = state.config.obs.as_ref().unwrap().port;
            let (changed, error) = common::validated_u16_input(
                ui,
                "Port:",
                &mut port,
                port_error.as_ref(),
                1,
                65535,
            );
            if changed {
                state.config.obs.as_mut().unwrap().port = port;
                state.mark_dirty();
                if let Some(err) = error {
                    state.set_error("obs.port", err);
                } else {
                    state.clear_error("obs.port");
                }
            }

            ui.add_space(5.0);

            // Password (optional)
            let mut password = state.config.obs.as_ref().unwrap().password.clone().unwrap_or_default();
            if common::password_input(ui, "Password:", &mut password) {
                state.config.obs.as_mut().unwrap().password = if password.is_empty() {
                    None
                } else {
                    Some(password)
                };
                state.mark_dirty();
            }

            ui.add_space(5.0);
        } else {
            ui.label(egui::RichText::new("OBS integration is disabled").italics().weak());
        }
    });
}
