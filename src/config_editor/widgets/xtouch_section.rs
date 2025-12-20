//! X-Touch hardware configuration section

use crate::config::{XTouchConfig, XTouchMode, OverlayConfig, OverlayMode, CcBits};
use crate::config_editor::state::EditorState;
use super::common;

/// Render X-Touch configuration section
pub fn render(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.collapsing("X-Touch Configuration", |ui| {
        ui.add_space(5.0);

        // Ensure xtouch config exists
        if state.config.xtouch.is_none() {
            state.config.xtouch = Some(XTouchConfig {
                mode: XTouchMode::Mcu,
                overlay: None,
                overlay_per_app: None,
            });
        }

        if state.config.xtouch.is_some() {
            // Mode selector
            let current_mode = state.config.xtouch.as_ref().unwrap().mode;
            ui.horizontal(|ui| {
                ui.label("Mode:");

                if ui.selectable_label(
                    current_mode == XTouchMode::Mcu,
                    "MCU (Recommended)"
                ).clicked() {
                    state.config.xtouch.as_mut().unwrap().mode = XTouchMode::Mcu;
                    state.mark_dirty();
                }

                if ui.selectable_label(
                    current_mode == XTouchMode::Ctrl,
                    "Ctrl"
                ).clicked() {
                    state.config.xtouch.as_mut().unwrap().mode = XTouchMode::Ctrl;
                    state.mark_dirty();
                }
            });

            ui.add_space(10.0);

            // Overlay configuration
            ui.label(egui::RichText::new("LCD Overlay:").strong());

            // Ensure overlay config exists
            let mut overlay_enabled = state.config.xtouch.as_ref().unwrap().overlay.is_some();
            if common::checkbox_input(ui, "Enable LCD Overlay", &mut overlay_enabled) {
                if overlay_enabled && state.config.xtouch.as_ref().unwrap().overlay.is_none() {
                    state.config.xtouch.as_mut().unwrap().overlay = Some(OverlayConfig {
                        enabled: true,
                        mode: Some(OverlayMode::Percent),
                        cc_bits: Some(CcBits::SevenBit),
                    });
                } else if !overlay_enabled {
                    state.config.xtouch.as_mut().unwrap().overlay = None;
                }
                state.mark_dirty();
            }

            if state.config.xtouch.as_ref().unwrap().overlay.is_some() {
                ui.indent("overlay_settings", |ui| {
                    ui.add_space(5.0);

                    // Overlay mode
                    let current_overlay_mode = state.config.xtouch.as_ref().unwrap()
                        .overlay.as_ref().unwrap().mode.unwrap_or(OverlayMode::Percent);

                    ui.horizontal(|ui| {
                        ui.label("Display Mode:");

                        if ui.selectable_label(
                            matches!(current_overlay_mode, OverlayMode::Percent),
                            "Percent (0-100%)"
                        ).clicked() {
                            state.config.xtouch.as_mut().unwrap()
                                .overlay.as_mut().unwrap().mode = Some(OverlayMode::Percent);
                            state.mark_dirty();
                        }

                        if ui.selectable_label(
                            matches!(current_overlay_mode, OverlayMode::SevenBit),
                            "7-bit (0-127)"
                        ).clicked() {
                            state.config.xtouch.as_mut().unwrap()
                                .overlay.as_mut().unwrap().mode = Some(OverlayMode::SevenBit);
                            state.mark_dirty();
                        }

                        if ui.selectable_label(
                            matches!(current_overlay_mode, OverlayMode::EightBit),
                            "8-bit (0-255)"
                        ).clicked() {
                            state.config.xtouch.as_mut().unwrap()
                                .overlay.as_mut().unwrap().mode = Some(OverlayMode::EightBit);
                            state.mark_dirty();
                        }
                    });

                    ui.add_space(5.0);

                    // CC bits mode
                    let current_cc_bits = state.config.xtouch.as_ref().unwrap()
                        .overlay.as_ref().unwrap().cc_bits.unwrap_or(CcBits::SevenBit);

                    ui.horizontal(|ui| {
                        ui.label("CC Bits:");

                        if ui.selectable_label(
                            matches!(current_cc_bits, CcBits::SevenBit),
                            "7-bit"
                        ).clicked() {
                            state.config.xtouch.as_mut().unwrap()
                                .overlay.as_mut().unwrap().cc_bits = Some(CcBits::SevenBit);
                            state.mark_dirty();
                        }

                        if ui.selectable_label(
                            matches!(current_cc_bits, CcBits::EightBit),
                            "8-bit"
                        ).clicked() {
                            state.config.xtouch.as_mut().unwrap()
                                .overlay.as_mut().unwrap().cc_bits = Some(CcBits::EightBit);
                            state.mark_dirty();
                        }
                    });
                });
            }
        }
    });
}
