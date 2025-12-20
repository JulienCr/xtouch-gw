//! Configuration Editor Module
//!
//! Provides a GUI-based configuration editor for XTouch GW v3 using egui/eframe.
//! Allows editing all config.yaml options with real-time validation.

mod app;
mod io;
mod state;
mod tabs;
mod validation;

pub mod widgets;

use anyhow::Result;
use app::ConfigEditorApp;
use state::EditorState;

/// Run the configuration editor in a separate window
///
/// This function spawns an eframe window with the config editor.
/// It loads the config from the specified path and allows editing with real-time validation.
///
/// # Arguments
/// * `config_path` - Path to the config.yaml file
///
/// # Returns
/// Result indicating success or error during window creation/config loading
pub fn run_config_editor(config_path: String) -> Result<()> {
    tracing::info!("Opening config editor for: {}", config_path);

    // Load config from file
    let config = io::load_config(&config_path)?;

    // Create editor state
    let state = EditorState::new(config, config_path);

    // Create egui application
    let app = ConfigEditorApp::new(state);

    // Configure eframe native options with Windows-specific settings
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("XTouch GW v3 - Configuration Editor")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([800.0, 600.0]),
        event_loop_builder: Some(Box::new(|builder| {
            // Allow event loop on any thread (Windows-specific)
            #[cfg(target_os = "windows")]
            {
                use winit::platform::windows::EventLoopBuilderExtWindows;
                builder.with_any_thread(true);
            }
        })),
        ..Default::default()
    };

    // Run the eframe application (blocking call)
    eframe::run_native(
        "XTouch GW v3 Config Editor",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {}", e))?;

    Ok(())
}
