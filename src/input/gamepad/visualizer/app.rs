//! Main application struct and polling logic for the gamepad visualizer.
//!
//! This module contains the [`GamepadVisualizerApp`] which implements eframe::App
//! and provides real-time visualization of connected gamepads. It supports two backends:
//!
//! - **XInput**: Native Windows API for Xbox-compatible controllers (4 fixed slots)
//! - **gilrs**: Cross-platform gamepad library for HID devices
//!
//! The app polls all controllers each frame and renders their state using the
//! rendering functions from the [`super::rendering`] module.

use super::super::visualizer_state::{ControllerBackend, VisualizerState};
use super::normalize::{
    normalize_stick_radial, normalize_trigger, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE,
    XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE,
};
use super::rendering::{render_buttons, render_controller_header, render_stick, render_trigger};
use gilrs::{ev::Code, Event, EventType, GamepadId, Gilrs};
use rusty_xinput::XInputHandle;
use std::collections::HashMap;

/// Main visualizer application supporting XInput and gilrs backends.
///
/// Maintains state for all connected controllers and renders their input
/// in real-time using egui. XInput controllers occupy fixed slots 0-3,
/// while gilrs controllers are tracked dynamically by their GamepadId.
pub struct GamepadVisualizerApp {
    state: VisualizerState,
    xinput: XInputHandle,
    gilrs: Gilrs,
    /// Track Capture button state per gamepad (gilrs reports it as Unknown)
    capture_state: HashMap<GamepadId, bool>,
}

impl GamepadVisualizerApp {
    /// Create a new visualizer app with both XInput and gilrs backends.
    ///
    /// # Panics
    ///
    /// Panics if XInput DLL cannot be loaded (Windows-only) or gilrs fails to initialize.
    pub fn new() -> Self {
        Self {
            state: VisualizerState::new(),
            xinput: XInputHandle::load_default().expect("Failed to load XInput DLL"),
            gilrs: Gilrs::new().expect("Failed to initialize gilrs"),
            capture_state: HashMap::new(),
        }
    }

    /// Poll all XInput controllers and update state.
    ///
    /// Iterates through user indices 0-3, updating connected controllers
    /// with raw and normalized stick/trigger values and trails.
    fn poll_xinput(&mut self) {
        for user_index in 0..4 {
            match self.xinput.get_state(user_index) {
                Ok(state) => {
                    // Normalize sticks with radial deadzone (circular, not square)
                    let (lx, ly) = normalize_stick_radial(
                        state.raw.Gamepad.sThumbLX,
                        state.raw.Gamepad.sThumbLY,
                        XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32,
                    );
                    let (rx, ry) = normalize_stick_radial(
                        state.raw.Gamepad.sThumbRX,
                        state.raw.Gamepad.sThumbRY,
                        XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE as f32,
                    );
                    let lt = normalize_trigger(state.left_trigger());
                    let rt = normalize_trigger(state.right_trigger());

                    // Update state with raw values
                    self.state.update_from_xinput(user_index, &state);

                    // Update normalized values and trails
                    if let Some(controller) =
                        self.state.xinput_controllers.get_mut(user_index as usize)
                    {
                        controller.left_stick.normalized_x = lx;
                        controller.left_stick.normalized_y = ly;
                        controller.right_stick.normalized_x = rx;
                        controller.right_stick.normalized_y = ry;
                        controller.left_trigger.normalized = lt;
                        controller.right_trigger.normalized = rt;

                        // Update trails with normalized values
                        controller.left_stick_trail.add_normalized_point(lx, ly);
                        controller.right_stick_trail.add_normalized_point(rx, ry);
                    }
                },
                Err(_) => {
                    self.state.mark_xinput_disconnected(user_index);
                },
            }
        }
    }

    /// Poll gilrs for events and update controller state.
    ///
    /// Processes connection/disconnection events and updates axis/button
    /// state for all connected non-XInput gamepads.
    fn poll_gilrs(&mut self) {
        // Process all pending events
        while let Some(Event { id, event, .. }) = self.gilrs.next_event() {
            match event {
                EventType::Connected => {
                    tracing::debug!("gilrs controller connected: {:?}", id);
                },
                EventType::Disconnected => {
                    self.state.remove_gilrs_controller(id);
                    self.capture_state.remove(&id);
                    tracing::debug!("gilrs controller disconnected: {:?}", id);
                },
                EventType::ButtonPressed(button, code) => {
                    tracing::debug!("gilrs button pressed: {:?} (code: {:?})", button, code);
                    // Track Capture button (Unknown, code 13)
                    if is_capture_button(&code) {
                        self.capture_state.insert(id, true);
                    }
                },
                EventType::ButtonReleased(button, code) => {
                    tracing::debug!("gilrs button released: {:?} (code: {:?})", button, code);
                    if is_capture_button(&code) {
                        self.capture_state.insert(id, false);
                    }
                },
                EventType::AxisChanged(axis, value, code) => {
                    // Only log significant axis changes to reduce noise
                    if value.abs() > 0.5 {
                        tracing::debug!("gilrs axis: {:?} = {:.2} (code: {:?})", axis, value, code);
                    }
                },
                _ => {},
            }
        }

        // Update state for all connected gilrs gamepads
        for (id, gamepad) in self.gilrs.gamepads() {
            // Skip if this looks like an XInput device to avoid duplicates
            // XInput devices typically support force feedback on Windows
            if gamepad.is_ff_supported() {
                continue;
            }

            // Get capture button state (tracked from events)
            let capture = self.capture_state.get(&id).copied().unwrap_or(false);

            // Update state from gilrs (already provides normalized values)
            self.state.update_from_gilrs(&gamepad, capture);

            // Update trails for this controller
            if let Some(controller) = self.state.gilrs_controllers.get_mut(&id) {
                // Get raw and normalized values for trail updates
                let raw_lx = controller.left_stick.gilrs_raw_x.unwrap_or(0.0);
                let raw_ly = controller.left_stick.gilrs_raw_y.unwrap_or(0.0);
                let raw_rx = controller.right_stick.gilrs_raw_x.unwrap_or(0.0);
                let raw_ry = controller.right_stick.gilrs_raw_y.unwrap_or(0.0);

                // Update raw trails (for square plot)
                controller.left_stick_trail.add_raw_point(raw_lx, raw_ly);
                controller.right_stick_trail.add_raw_point(raw_rx, raw_ry);

                // Update normalized trails (for circle plot)
                controller.left_stick_trail.add_normalized_point(
                    controller.left_stick.normalized_x,
                    controller.left_stick.normalized_y,
                );
                controller.right_stick_trail.add_normalized_point(
                    controller.right_stick.normalized_x,
                    controller.right_stick.normalized_y,
                );
            }
        }
    }

    /// Get count of connected controllers across both backends.
    fn connected_count(&self) -> usize {
        let xinput_count = self
            .state
            .xinput_controllers
            .iter()
            .filter(|c| c.connected)
            .count();
        let gilrs_count = self.state.gilrs_controllers.len();
        xinput_count + gilrs_count
    }
}

impl Default for GamepadVisualizerApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for GamepadVisualizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll both backends every frame (~60Hz)
        self.poll_xinput();
        self.poll_gilrs();

        // Handle numpad 5 key to clear all trails
        if ctx.input(|i| i.key_pressed(egui::Key::Num5)) {
            self.state.clear_all_trails();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(
                egui::RichText::new("Gamepad Visualizer")
                    .size(20.0)
                    .strong(),
            );

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("XInput + HID Debug Tool - Raw and Normalized Values")
                        .color(egui::Color32::from_gray(180))
                        .size(12.0),
                );
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new("Press Numpad 5 to clear trails")
                        .color(egui::Color32::from_rgb(150, 150, 200))
                        .size(11.0),
                );
            });

            ui.add_space(12.0);

            // Check if any controllers are connected
            if self.connected_count() == 0 {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(
                        egui::RichText::new("No controllers detected")
                            .size(16.0)
                            .color(egui::Color32::from_rgb(255, 200, 100)),
                    );
                    ui.label(
                        egui::RichText::new(
                            "Please connect an XInput controller (Xbox) or HID gamepad",
                        )
                        .color(egui::Color32::from_gray(150)),
                    );
                });
            }

            // Show connected controllers
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Render XInput controllers
                for controller in &self.state.xinput_controllers {
                    if !controller.connected {
                        continue;
                    }

                    render_controller_group(ui, controller);
                    ui.add_space(12.0);
                }

                // Render gilrs controllers
                for controller in self.state.gilrs_controllers.values() {
                    render_controller_group(ui, controller);
                    ui.add_space(12.0);
                }
            });
        });

        // Request repaint for next frame
        ctx.request_repaint();
    }
}

/// Render a single controller's complete UI group.
fn render_controller_group(
    ui: &mut egui::Ui,
    controller: &super::super::visualizer_state::ControllerState,
) {
    ui.group(|ui| {
        ui.set_min_width(ui.available_width());

        // Controller header with backend-specific info
        let title = match &controller.backend {
            ControllerBackend::XInput { user_index, .. } => {
                format!("Controller {} (XInput)", user_index + 1)
            },
            ControllerBackend::Gilrs { name, .. } => {
                format!("Controller ({})", name)
            },
        };

        ui.heading(
            egui::RichText::new(title)
                .size(16.0)
                .color(egui::Color32::from_rgb(150, 200, 255)),
        );

        render_controller_header(ui, controller);

        ui.add_space(8.0);

        // Sticks side by side
        // XInput shows deadzone circle, Gilrs does not (driver handles deadzone)
        let (left_dz, right_dz) = match &controller.backend {
            ControllerBackend::XInput { .. } => (
                Some(XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE),
                Some(XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE),
            ),
            ControllerBackend::Gilrs { .. } => (None, None),
        };

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                render_stick(
                    ui,
                    "Left Stick",
                    &controller.left_stick,
                    &controller.left_stick_trail,
                    left_dz,
                );
            });

            ui.add_space(16.0);

            ui.vertical(|ui| {
                render_stick(
                    ui,
                    "Right Stick",
                    &controller.right_stick,
                    &controller.right_stick_trail,
                    right_dz,
                );
            });
        });

        ui.add_space(12.0);

        // Triggers
        ui.label(egui::RichText::new("Triggers").strong().size(14.0));
        render_trigger(
            ui,
            "LT:",
            &controller.left_trigger,
            egui::Color32::from_rgb(100, 150, 255),
        );
        render_trigger(
            ui,
            "RT:",
            &controller.right_trigger,
            egui::Color32::from_rgb(255, 150, 100),
        );

        ui.add_space(12.0);

        // Buttons
        render_buttons(ui, controller);
    });
}

/// Button index for Capture button (not mapped by gilrs standard).
///
/// The gilrs library does not include the Capture button in its standard `Button` enum.
/// On controllers like the Nintendo Switch Pro Controller or FaceOff, this button is
/// reported as `Button::Unknown` with a raw code containing "index: 13".
const CAPTURE_BUTTON_INDEX: &str = "index: 13";

/// Check if a button code is the Capture button (button index 13).
///
/// # Implementation Note
///
/// This is a **fragile workaround** because gilrs's `Code` type does not expose its
/// internal button index publicly. We resort to parsing the `Debug` output string,
/// which may break if gilrs changes its Debug formatting in future versions.
///
/// **Recommendation**: Pin the gilrs version in Cargo.toml and test after upgrades.
/// If this breaks, check the new Debug format and update `CAPTURE_BUTTON_INDEX`.
///
/// The Capture button is reported as `Button::Unknown` with code containing "index: 13".
fn is_capture_button(code: &Code) -> bool {
    let code_str = format!("{:?}", code);
    let has_index = code_str.contains(CAPTURE_BUTTON_INDEX);
    let has_button = code_str.contains("Button");

    // Runtime validation: if we see the expected index but the Debug output no longer
    // contains "Button", log the full Code format so changes in gilrs formatting
    // can be noticed and this workaround can be updated accordingly.
    if has_index && !has_button {
        tracing::warn!(
            "Possible Capture button with unexpected gilrs::ev::Code Debug format: {:?}",
            code
        );
    }

    has_index && has_button
}
