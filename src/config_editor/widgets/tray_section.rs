//! System tray UI configuration section

use crate::config::TrayConfig;
use crate::config_editor::state::EditorState;
use super::common;

/// Render tray configuration section
pub fn render(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.collapsing("System Tray Configuration", |ui| {
        ui.add_space(5.0);

        // Ensure tray config exists
        if state.config.tray.is_none() {
            state.config.tray = Some(TrayConfig {
                enabled: true,
                activity_led_duration_ms: 200,
                status_poll_interval_ms: 100,
                show_activity_leds: true,
                show_connection_status: true,
            });
        }

        if state.config.tray.is_some() {
            // Enabled checkbox
            let mut enabled = state.config.tray.as_ref().unwrap().enabled;
            if common::checkbox_input(ui, "Enable System Tray Icon", &mut enabled) {
                state.config.tray.as_mut().unwrap().enabled = enabled;
                state.mark_dirty();
            }

            ui.add_space(10.0);

            // Cache the enabled state
            let tray_enabled = state.config.tray.as_ref().unwrap().enabled;

            // Settings (only if tray is enabled)
            if tray_enabled {
                // Activity LED duration
                let mut duration = state.config.tray.as_ref().unwrap().activity_led_duration_ms as i32;
                ui.horizontal(|ui| {
                    ui.label("Activity LED Duration (ms):");
                    let response = ui.add(
                        egui::DragValue::new(&mut duration)
                            .range(50..=5000)
                            .speed(10)
                    );

                    if response.changed() {
                        state.config.tray.as_mut().unwrap().activity_led_duration_ms = duration as u64;
                        state.mark_dirty();
                    }
                });

                ui.add_space(5.0);

                // Status poll interval
                let mut interval = state.config.tray.as_ref().unwrap().status_poll_interval_ms as i32;
                ui.horizontal(|ui| {
                    ui.label("Status Poll Interval (ms):");
                    let response = ui.add(
                        egui::DragValue::new(&mut interval)
                            .range(50..=1000)
                            .speed(10)
                    );

                    if response.changed() {
                        state.config.tray.as_mut().unwrap().status_poll_interval_ms = interval as u64;
                        state.mark_dirty();
                    }
                });

                ui.add_space(10.0);

                // Display options
                let mut show_activity_leds = state.config.tray.as_ref().unwrap().show_activity_leds;
                if common::checkbox_input(
                    ui,
                    "Show Activity LEDs",
                    &mut show_activity_leds
                ) {
                    state.config.tray.as_mut().unwrap().show_activity_leds = show_activity_leds;
                    state.mark_dirty();
                }

                ui.add_space(5.0);

                let mut show_connection_status = state.config.tray.as_ref().unwrap().show_connection_status;
                if common::checkbox_input(
                    ui,
                    "Show Connection Status",
                    &mut show_connection_status
                ) {
                    state.config.tray.as_mut().unwrap().show_connection_status = show_connection_status;
                    state.mark_dirty();
                }

                ui.add_space(5.0);

                ui.label(
                    egui::RichText::new("ℹ Changes to tray settings require application restart")
                        .italics()
                        .weak()
                        .small()
                );
            }
        }
    });
}
