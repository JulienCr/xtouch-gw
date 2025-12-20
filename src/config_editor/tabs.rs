//! Tab management
//!
//! Handles rendering of tab bar and tab switching logic.

use super::state::{EditorState, EditorTab};

/// Render tab bar and handle tab switching
pub fn render_tab_bar(ui: &mut egui::Ui, state: &mut EditorState) {
    ui.horizontal(|ui| {
        // General tab
        if ui.selectable_label(
            state.active_tab == EditorTab::General,
            "General"
        ).clicked() {
            state.active_tab = EditorTab::General;
        }

        // Gamepad tab
        if ui.selectable_label(
            state.active_tab == EditorTab::Gamepad,
            "Gamepad"
        ).clicked() {
            state.active_tab = EditorTab::Gamepad;
        }

        // Global Defaults tab
        if ui.selectable_label(
            state.active_tab == EditorTab::GlobalDefaults,
            "Global Defaults"
        ).clicked() {
            state.active_tab = EditorTab::GlobalDefaults;
        }

        ui.separator();

        // Page tabs (1-8)
        for i in 0..8 {
            let page_label = format!("Page {}", i + 1);
            if ui.selectable_label(
                state.active_tab == EditorTab::Page(i),
                &page_label
            ).clicked() {
                state.active_tab = EditorTab::Page(i);
                state.active_page_idx = i;
            }
        }
    });
}
