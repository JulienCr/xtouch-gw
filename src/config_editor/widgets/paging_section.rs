//! Paging configuration section (F1-F8 page navigation)

use crate::config::PagingConfig;
use crate::config_editor::state::EditorState;
use super::common;

/// Render paging configuration section
pub fn render(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.collapsing("Paging Configuration", |ui| {
        ui.add_space(5.0);

        // Enable/disable checkbox
        let mut enabled = state.config.paging.is_some();
        if common::checkbox_input(ui, "Enable Page Navigation (F1-F8)", &mut enabled) {
            if enabled && state.config.paging.is_none() {
                // Create default paging config
                state.config.paging = Some(PagingConfig {
                    channel: 1,
                    prev_note: 46,
                    next_note: 47,
                });
            } else if !enabled {
                state.config.paging = None;
            }
            state.mark_dirty();
        }

        ui.add_space(5.0);

        // Only show settings if enabled
        let paging_enabled = state.config.paging.is_some();
        if paging_enabled {
            // MIDI channel - get error and value first
            let channel_error = state.get_error("paging.channel");
            let mut channel = state.config.paging.as_ref().unwrap().channel;
            let (changed, error) = common::validated_u8_input(
                ui,
                "MIDI Channel:",
                &mut channel,
                channel_error.as_ref(),
                1,
                16,
            );
            if changed {
                state.config.paging.as_mut().unwrap().channel = channel;
                state.mark_dirty();
                if let Some(err) = error {
                    state.set_error("paging.channel", err);
                } else {
                    state.clear_error("paging.channel");
                }
            }

            ui.add_space(5.0);

            // Previous page note
            let prev_error = state.get_error("paging.prev_note");
            let mut prev_note = state.config.paging.as_ref().unwrap().prev_note;
            let (changed, error) = common::validated_u8_input(
                ui,
                "Previous Page Note:",
                &mut prev_note,
                prev_error.as_ref(),
                0,
                127,
            );
            if changed {
                state.config.paging.as_mut().unwrap().prev_note = prev_note;
                state.mark_dirty();
                if let Some(err) = error {
                    state.set_error("paging.prev_note", err);
                } else {
                    state.clear_error("paging.prev_note");
                }
            }

            ui.add_space(5.0);

            // Next page note
            let next_error = state.get_error("paging.next_note");
            let mut next_note = state.config.paging.as_ref().unwrap().next_note;
            let (changed, error) = common::validated_u8_input(
                ui,
                "Next Page Note:",
                &mut next_note,
                next_error.as_ref(),
                0,
                127,
            );
            if changed {
                state.config.paging.as_mut().unwrap().next_note = next_note;
                state.mark_dirty();
                if let Some(err) = error {
                    state.set_error("paging.next_note", err);
                } else {
                    state.clear_error("paging.next_note");
                }
            }

            ui.add_space(5.0);

            ui.label(
                egui::RichText::new("ℹ Default: Channel 1, Prev=46, Next=47")
                    .italics()
                    .weak()
                    .small()
            );
        } else {
            ui.label(egui::RichText::new("Page navigation is disabled").italics().weak());
        }
    });
}
