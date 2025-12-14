//! Gamepad Visualizer - Native GUI for debugging XInput values
//!
//! Displays raw and normalized stick/trigger values, button states, and timing info
//! for all connected XInput controllers. Useful for debugging axis boundary issues.

use super::visualizer_state::{VisualizerState, ControllerState, StickState, TriggerState};
use rusty_xinput::XInputHandle;

/// Main visualizer application
pub struct GamepadVisualizerApp {
    state: VisualizerState,
    xinput: XInputHandle,
}

impl GamepadVisualizerApp {
    /// Create new visualizer app
    pub fn new() -> Self {
        Self {
            state: VisualizerState::new(),
            xinput: XInputHandle::load_default().expect("Failed to load XInput DLL"),
        }
    }

    /// Poll all XInput controllers and update state
    fn poll_xinput(&mut self) {
        for user_index in 0..4 {
            match self.xinput.get_state(user_index) {
                Ok(state) => {
                    // Normalize values with Microsoft-recommended deadzones
                    let lx = normalize_stick(state.raw.Gamepad.sThumbLX, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE);
                    let ly = normalize_stick(state.raw.Gamepad.sThumbLY, XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE);
                    let rx = normalize_stick(state.raw.Gamepad.sThumbRX, XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE);
                    let ry = normalize_stick(state.raw.Gamepad.sThumbRY, XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE);
                    let lt = normalize_trigger(state.left_trigger());
                    let rt = normalize_trigger(state.right_trigger());

                    // Update state with raw values
                    self.state.update_from_xinput(user_index, &state);

                    // Update normalized values
                    if let Some(controller) = self.state.controllers.get_mut(user_index as usize) {
                        controller.left_stick.normalized_x = lx;
                        controller.left_stick.normalized_y = ly;
                        controller.right_stick.normalized_x = rx;
                        controller.right_stick.normalized_y = ry;
                        controller.left_trigger.normalized = lt;
                        controller.right_trigger.normalized = rt;
                    }
                }
                Err(_) => {
                    self.state.mark_disconnected(user_index);
                }
            }
        }
    }

    /// Render a stick visualization
    fn render_stick(&self, ui: &mut egui::Ui, label: &str, stick: &StickState, deadzone: i16) {
        ui.label(egui::RichText::new(label).strong().size(14.0));

        // Visual 2D representation
        let (response, painter) = ui.allocate_painter(
            egui::Vec2::new(200.0, 200.0),
            egui::Sense::hover(),
        );

        let rect = response.rect;
        let center = rect.center();
        let radius = 90.0;

        // Background circle (darker)
        painter.circle_filled(center, radius, egui::Color32::from_gray(30));

        // Deadzone circle (varies by stick: left=7849/32768≈0.24, right=8689/32768≈0.265)
        let deadzone_ratio = deadzone as f32 / 32768.0;
        let deadzone_radius = radius * deadzone_ratio;
        painter.circle_filled(center, deadzone_radius, egui::Color32::from_gray(50));

        // Border for background
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
        );

        // Center crosshair (reference)
        let crosshair_size = 3.0;
        painter.line_segment(
            [
                egui::pos2(center.x - crosshair_size, center.y),
                egui::pos2(center.x + crosshair_size, center.y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
        );
        painter.line_segment(
            [
                egui::pos2(center.x, center.y - crosshair_size),
                egui::pos2(center.x, center.y + crosshair_size),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
        );

        // Current position (normalized -1..1 → pixel coordinates)
        let pos_x = center.x + stick.normalized_x * radius;
        let pos_y = center.y - stick.normalized_y * radius; // Flip Y (screen coords)

        // Position crosshair (red)
        let cross_size = 6.0;
        painter.line_segment(
            [
                egui::pos2(pos_x - cross_size, pos_y),
                egui::pos2(pos_x + cross_size, pos_y),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 80, 80)),
        );
        painter.line_segment(
            [
                egui::pos2(pos_x, pos_y - cross_size),
                egui::pos2(pos_x, pos_y + cross_size),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 80, 80)),
        );

        // Position indicator dot
        painter.circle_filled(
            egui::pos2(pos_x, pos_y),
            3.0,
            egui::Color32::from_rgb(255, 100, 100),
        );

        // Text values below
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Raw:").color(egui::Color32::from_gray(180)));
            ui.label(
                egui::RichText::new(format!("({:6}, {:6})", stick.raw_x, stick.raw_y))
                    .color(egui::Color32::from_rgb(150, 200, 255))
                    .family(egui::FontFamily::Monospace),
            );
        });

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Norm:").color(egui::Color32::from_gray(180)));
            ui.label(
                egui::RichText::new(format!(
                    "({:6.3}, {:6.3})",
                    stick.normalized_x, stick.normalized_y
                ))
                .color(egui::Color32::from_rgb(150, 255, 150))
                .family(egui::FontFamily::Monospace),
            );
        });

        ui.add_space(8.0);
    }

    /// Render trigger visualization
    fn render_trigger(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        trigger: &TriggerState,
        color: egui::Color32,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(label)
                    .strong()
                    .size(12.0)
                    .color(egui::Color32::from_gray(200)),
            );

            // Progress bar (0.0 to 1.0)
            let progress = trigger.normalized;
            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(180.0)
                    .fill(color),
            );

            // Raw value
            ui.label(
                egui::RichText::new(format!("{:3}", trigger.raw))
                    .color(egui::Color32::from_rgb(150, 200, 255))
                    .family(egui::FontFamily::Monospace),
            );

            // Normalized value
            ui.label(
                egui::RichText::new(format!("{:5.3}", trigger.normalized))
                    .color(egui::Color32::from_rgb(150, 255, 150))
                    .family(egui::FontFamily::Monospace),
            );
        });
    }

    /// Render button grid
    fn render_buttons(&self, ui: &mut egui::Ui, controller: &ControllerState) {
        ui.label(egui::RichText::new("Buttons").strong().size(14.0));

        ui.horizontal_wrapped(|ui| {
            for (name, pressed) in controller.buttons.iter() {
                let color = if pressed {
                    egui::Color32::from_rgb(100, 255, 100)
                } else {
                    egui::Color32::from_gray(60)
                };

                let text_color = if pressed {
                    egui::Color32::BLACK
                } else {
                    egui::Color32::from_gray(150)
                };

                let button = egui::Button::new(
                    egui::RichText::new(name)
                        .color(text_color)
                        .size(11.0)
                        .family(egui::FontFamily::Monospace),
                )
                .fill(color)
                .min_size(egui::vec2(55.0, 24.0));

                ui.add(button);
            }
        });

        ui.add_space(8.0);
    }

    /// Render controller info header
    fn render_controller_header(&self, ui: &mut egui::Ui, controller: &ControllerState) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("XInput User Index: {}", controller.user_index))
                    .color(egui::Color32::from_gray(200))
                    .size(12.0),
            );

            ui.label(
                egui::RichText::new(format!("| Packet: {}", controller.packet_number))
                    .color(egui::Color32::from_gray(150))
                    .size(11.0)
                    .family(egui::FontFamily::Monospace),
            );

            let elapsed = controller.last_update.elapsed().as_millis();
            ui.label(
                egui::RichText::new(format!("| Last update: {}ms ago", elapsed))
                    .color(egui::Color32::from_gray(150))
                    .size(11.0)
                    .family(egui::FontFamily::Monospace),
            );
        });

        ui.separator();
    }
}

impl eframe::App for GamepadVisualizerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll XInput every frame (~60Hz)
        self.poll_xinput();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(
                egui::RichText::new("🎮 Gamepad Visualizer")
                    .size(20.0)
                    .strong(),
            );

            ui.label(
                egui::RichText::new("XInput Debug Tool - Raw and Normalized Values")
                    .color(egui::Color32::from_gray(180))
                    .size(12.0),
            );

            ui.add_space(12.0);

            // Check if any controllers are connected
            let connected_count = self
                .state
                .controllers
                .iter()
                .filter(|c| c.connected)
                .count();

            if connected_count == 0 {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(
                        egui::RichText::new("⚠ No controllers detected")
                            .size(16.0)
                            .color(egui::Color32::from_rgb(255, 200, 100)),
                    );
                    ui.label(
                        egui::RichText::new("Please connect an XInput controller (Xbox, etc.)")
                            .color(egui::Color32::from_gray(150)),
                    );
                });
            }

            // Show connected controllers
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (idx, controller) in self.state.controllers.iter().enumerate() {
                    if !controller.connected {
                        continue;
                    }

                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());

                        // Controller header
                        ui.heading(
                            egui::RichText::new(format!("Controller {}", idx + 1))
                                .size(16.0)
                                .color(egui::Color32::from_rgb(150, 200, 255)),
                        );

                        self.render_controller_header(ui, controller);

                        ui.add_space(8.0);

                        // Sticks side by side
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                self.render_stick(
                                    ui,
                                    "Left Stick",
                                    &controller.left_stick,
                                    XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE,
                                );
                            });

                            ui.add_space(16.0);

                            ui.vertical(|ui| {
                                self.render_stick(
                                    ui,
                                    "Right Stick",
                                    &controller.right_stick,
                                    XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE,
                                );
                            });
                        });

                        ui.add_space(12.0);

                        // Triggers
                        ui.label(egui::RichText::new("Triggers").strong().size(14.0));
                        self.render_trigger(
                            ui,
                            "LT:",
                            &controller.left_trigger,
                            egui::Color32::from_rgb(100, 150, 255),
                        );
                        self.render_trigger(
                            ui,
                            "RT:",
                            &controller.right_trigger,
                            egui::Color32::from_rgb(255, 150, 100),
                        );

                        ui.add_space(12.0);

                        // Buttons
                        self.render_buttons(ui, controller);
                    });

                    ui.add_space(12.0);
                }
            });
        });

        // Request repaint for next frame
        ctx.request_repaint();
    }
}

/// Entry point for gamepad visualizer
///
/// Called from main.rs when --gamepad-diagnostics flag is set.
/// Blocks until window is closed.
pub fn run_visualizer() {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gamepad Visualizer - XInput Debug")
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

// ============================================================================
// Normalization Functions with Microsoft-recommended deadzones
// ============================================================================

/// Microsoft's official XInput deadzone recommendations
const XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE: i16 = 7849;
const XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE: i16 = 8689;
const XINPUT_GAMEPAD_TRIGGER_THRESHOLD: u8 = 30;

/// Normalize XInput stick value (i16) to -1.0 to 1.0
///
/// Applies XInput's recommended deadzone at the hardware level,
/// then rescales to use the full -1.0 to 1.0 range for the remaining motion.
///
/// # Arguments
/// * `value` - Raw stick value from XInput (-32768 to 32767)
/// * `deadzone` - Deadzone threshold (7849 for left stick, 8689 for right stick)
fn normalize_stick(value: i16, deadzone: i16) -> f32 {
    // Use i32 for absolute value to correctly handle i16::MIN (-32768)
    // wrapping_abs() would return -32768 for i16::MIN, causing full left/down to return 0
    let abs_value = (value as i32).abs();

    if abs_value < deadzone as i32 {
        return 0.0;
    }

    // Rescale to -1.0..1.0 accounting for asymmetric range and deadzone
    // Negative: -32768 to -deadzone = 32768 - deadzone values
    // Positive: +deadzone to +32767 = 32767 - deadzone values
    let available_range = if value < 0 {
        32768.0 - deadzone as f32
    } else {
        32767.0 - deadzone as f32
    };

    let adjusted_value = if value < 0 {
        value + deadzone
    } else {
        value - deadzone
    };

    adjusted_value as f32 / available_range
}

/// Normalize XInput trigger value (u8) to 0.0 to 1.0
///
/// Applies Microsoft's recommended trigger threshold and rescales to 0.0-1.0 range.
fn normalize_trigger(value: u8) -> f32 {
    if value < XINPUT_GAMEPAD_TRIGGER_THRESHOLD {
        return 0.0;
    }

    let adjusted = value - XINPUT_GAMEPAD_TRIGGER_THRESHOLD;
    let range = 255 - XINPUT_GAMEPAD_TRIGGER_THRESHOLD;
    adjusted as f32 / range as f32
}
