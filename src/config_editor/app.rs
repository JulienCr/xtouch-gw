//! Main config editor application
//!
//! Implements eframe::App for the configuration editor GUI.

use super::{io, state::{EditorState, EditorTab}, tabs};
use egui;

/// Main configuration editor application
pub struct ConfigEditorApp {
    state: EditorState,
}

impl ConfigEditorApp {
    /// Create new config editor app
    pub fn new(state: EditorState) -> Self {
        Self { state }
    }

    /// Render the General tab (MIDI, OBS, X-Touch, Paging, Tray)
    fn render_general_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("General Settings");
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            // MIDI section
            super::widgets::midi_section::render(ui, &mut self.state);
            ui.add_space(10.0);

            // OBS section
            super::widgets::obs_section::render(ui, &mut self.state);
            ui.add_space(10.0);

            // X-Touch section
            super::widgets::xtouch_section::render(ui, &mut self.state);
            ui.add_space(10.0);

            // Paging section
            super::widgets::paging_section::render(ui, &mut self.state);
            ui.add_space(10.0);

            // Tray section
            super::widgets::tray_section::render(ui, &mut self.state);
            ui.add_space(10.0);
        });
    }

    /// Render the Gamepad tab
    fn render_gamepad_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            super::widgets::gamepad_section::render(ui, &mut self.state);
        });
    }

    /// Render the Global Defaults tab
    fn render_global_defaults_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Global Page Defaults");
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("🚧 Global defaults section will be implemented in Phase 4");
        });
    }

    /// Render a Page tab (1-8)
    fn render_page_tab(&mut self, ui: &mut egui::Ui, page_idx: usize) {
        ui.heading(format!("Page {} Configuration", page_idx + 1));
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("🚧 Page editor will be implemented in Phase 5");
        });
    }

    /// Render bottom status bar with error count and action buttons
    fn render_status_bar(&mut self, ui: &mut egui::Ui) -> bool {
        let mut should_close = false;

        ui.horizontal(|ui| {
            // Left side: error status
            if self.state.has_errors() {
                ui.label(
                    egui::RichText::new(format!("⚠ {} errors", self.state.error_count()))
                        .color(egui::Color32::from_rgb(255, 100, 100))
                );
            } else if self.state.has_unsaved_changes() {
                ui.label(
                    egui::RichText::new("● Unsaved changes")
                        .color(egui::Color32::from_rgb(255, 200, 100))
                );
            } else {
                ui.label(
                    egui::RichText::new("✓ No errors")
                        .color(egui::Color32::from_rgb(100, 255, 100))
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Close button
                if ui.button("Close").clicked() {
                    if self.state.has_unsaved_changes() {
                        self.state.show_unsaved_warning = true;
                    } else {
                        should_close = true;
                    }
                }

                // Save button (disabled if errors exist)
                let save_button = ui.add_enabled(
                    !self.state.has_errors(),
                    egui::Button::new("Save")
                );

                if save_button.clicked() {
                    self.save_config();
                }

                // Reload button
                if ui.button("Reload").clicked() {
                    self.reload_config();
                }
            });
        });

        should_close
    }

    /// Save configuration to file
    fn save_config(&mut self) {
        match io::save_config(&self.state.config, &self.state.config_path) {
            Ok(_) => {
                self.state.mark_clean();
                tracing::info!("Config saved successfully");
            }
            Err(e) => {
                tracing::error!("Failed to save config: {}", e);
                // TODO: Show error dialog in Phase 6
            }
        }
    }

    /// Reload configuration from file
    fn reload_config(&mut self) {
        match io::load_config(&self.state.config_path) {
            Ok(config) => {
                self.state.config = config;
                self.state.mark_clean();
                self.state.clear_all_errors();
                tracing::info!("Config reloaded successfully");
            }
            Err(e) => {
                tracing::error!("Failed to reload config: {}", e);
                // TODO: Show error dialog in Phase 6
            }
        }
    }

    /// Render unsaved changes warning dialog
    fn render_unsaved_warning(&mut self, ctx: &egui::Context) -> bool {
        let mut should_close = false;

        if self.state.show_unsaved_warning {
            egui::Window::new("Unsaved Changes")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("You have unsaved changes. Do you want to close without saving?");
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        if ui.button("Discard and Close").clicked() {
                            should_close = true;
                            self.state.show_unsaved_warning = false;
                        }

                        if ui.button("Keep Editing").clicked() {
                            self.state.show_unsaved_warning = false;
                        }
                    });
                });
        }

        should_close
    }
}

impl eframe::App for ConfigEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check for unsaved changes warning
        if self.render_unsaved_warning(ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Top panel: Title and unsaved indicator
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("XTouch GW v3 - Configuration Editor");

                if self.state.has_unsaved_changes() {
                    ui.label(
                        egui::RichText::new("[Unsaved]")
                            .color(egui::Color32::from_rgb(255, 200, 100))
                    );
                }
            });
        });

        // Bottom panel: Status bar with error count and action buttons
        let should_close = egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            self.render_status_bar(ui)
        }).inner;

        if should_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Central panel: Tab bar and content
        egui::CentralPanel::default().show(ctx, |ui| {
            // Tab bar
            tabs::render_tab_bar(ui, &mut self.state);
            ui.separator();

            // Tab content based on active tab
            match self.state.active_tab {
                EditorTab::General => self.render_general_tab(ui),
                EditorTab::Gamepad => self.render_gamepad_tab(ui),
                EditorTab::GlobalDefaults => self.render_global_defaults_tab(ui),
                EditorTab::Page(idx) => self.render_page_tab(ui, idx),
            }
        });

        // Handle window close request (X button)
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.state.has_unsaved_changes() {
                self.state.show_unsaved_warning = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
    }
}
