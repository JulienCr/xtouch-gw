//! Gamepad Visualizer - Native GUI for debugging gamepad values
//!
//! Supports both XInput (Xbox) and HID (generic) controllers via gilrs.
//! Displays raw and normalized stick/trigger values, button states, and timing info.

mod app;
pub mod normalize;
mod rendering;

pub use app::GamepadVisualizerApp;

/// Entry point for gamepad visualizer
///
/// Called from main.rs when --gamepad-diagnostics flag is set.
/// Blocks until window is closed.
pub fn run_visualizer() {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gamepad Visualizer - XInput & HID Debug")
            .with_inner_size([950.0, 750.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Gamepad Visualizer",
        native_options,
        Box::new(|_cc| Ok(Box::new(GamepadVisualizerApp::new()))),
    );
}
