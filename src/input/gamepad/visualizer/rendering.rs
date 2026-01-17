//! Rendering functions for gamepad visualizer UI
//!
//! Contains all egui rendering logic extracted from the main visualizer module.
//! Handles stick visualizations, trigger bars, button grids, and controller headers
//! for both XInput and gilrs backends.

use super::super::visualizer_state::{
    ControllerBackend, ControllerState, StickState, StickTrail, TriggerState,
};

/// Render a stick visualization with optional deadzone display
///
/// Displays:
/// - Background circle with optional deadzone ring
/// - Center crosshair reference
/// - Trail showing stick movement history
/// - Current position indicator (red crosshair + dot)
/// - Raw values (if available, otherwise "N/A")
/// - Normalized values
/// - Magnitude calculation
///
/// For gilrs controllers, shows dual plots (raw vs normalized) side by side.
pub fn render_stick(
    ui: &mut egui::Ui,
    label: &str,
    stick: &StickState,
    trail: &StickTrail,
    deadzone: Option<i16>,
) {
    ui.label(egui::RichText::new(label).strong().size(14.0));

    // Check if this is a gilrs controller with raw float values
    let has_gilrs_raw = stick.gilrs_raw_x.is_some() && stick.gilrs_raw_y.is_some();

    if has_gilrs_raw {
        // Gilrs controller: show dual plots (raw vs normalized)
        render_gilrs_dual_stick(ui, stick, trail);
    } else {
        // XInput controller: show single plot with deadzone
        render_xinput_stick(ui, stick, trail, deadzone);
    }
}

/// Render XInput stick with deadzone visualization (single plot)
fn render_xinput_stick(
    ui: &mut egui::Ui,
    stick: &StickState,
    trail: &StickTrail,
    deadzone: Option<i16>,
) {
    let (response, painter) =
        ui.allocate_painter(egui::Vec2::new(200.0, 200.0), egui::Sense::hover());

    let rect = response.rect;
    let center = rect.center();
    let radius = 90.0;

    // Background circle (darker)
    painter.circle_filled(center, radius, egui::Color32::from_gray(30));

    // Deadzone circle (only if deadzone is provided and > 0)
    if let Some(dz) = deadzone {
        if dz > 0 {
            let deadzone_ratio = dz as f32 / 32768.0;
            let deadzone_radius = radius * deadzone_ratio;
            painter.circle_filled(center, deadzone_radius, egui::Color32::from_gray(50));
        }
    }

    // Border for background
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
    );

    draw_crosshair(&painter, center, 3.0, egui::Color32::from_gray(80));

    // Draw trail (before position marker so it appears behind)
    draw_trail(
        &painter,
        center,
        radius,
        &trail.normalized_points,
        egui::Color32::from_rgba_unmultiplied(255, 80, 80, 100), // Semi-transparent red
    );

    // Draw normalized position
    draw_position_marker(
        &painter,
        center,
        radius,
        stick.normalized_x,
        stick.normalized_y,
        egui::Color32::from_rgb(255, 80, 80),
    );

    // Text values below - Raw values (XInput i16)
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Raw:").color(egui::Color32::from_gray(180)));
        match (stick.raw_x, stick.raw_y) {
            (Some(rx), Some(ry)) => {
                ui.label(
                    egui::RichText::new(format!("({:6}, {:6})", rx, ry))
                        .color(egui::Color32::from_rgb(150, 200, 255))
                        .family(egui::FontFamily::Monospace),
                );
            },
            _ => {
                ui.label(
                    egui::RichText::new("N/A")
                        .color(egui::Color32::from_gray(100))
                        .family(egui::FontFamily::Monospace),
                );
            },
        }
    });

    render_normalized_values(ui, stick);
    render_magnitude(ui, stick.normalized_x, stick.normalized_y);
    ui.add_space(8.0);
}

/// Render gilrs stick with dual plots: raw (square) vs normalized (circle)
fn render_gilrs_dual_stick(ui: &mut egui::Ui, stick: &StickState, trail: &StickTrail) {
    let raw_x = stick.gilrs_raw_x.unwrap_or(0.0);
    let raw_y = stick.gilrs_raw_y.unwrap_or(0.0);
    let raw_magnitude = (raw_x * raw_x + raw_y * raw_y).sqrt();
    let norm_magnitude =
        (stick.normalized_x * stick.normalized_x + stick.normalized_y * stick.normalized_y).sqrt();

    // Two plots side by side
    ui.horizontal(|ui| {
        // Left plot: Raw gilrs values (square boundary)
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("Raw (gilrs)")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(150, 200, 255)),
            );
            render_square_plot(ui, raw_x, raw_y, &trail.raw_points);
        });

        ui.add_space(8.0);

        // Right plot: Normalized values (circle boundary)
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("Normalized")
                    .size(11.0)
                    .color(egui::Color32::from_rgb(150, 255, 150)),
            );
            render_circle_plot(
                ui,
                stick.normalized_x,
                stick.normalized_y,
                &trail.normalized_points,
            );
        });
    });

    // Values display
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Raw:").color(egui::Color32::from_gray(180)));
        ui.label(
            egui::RichText::new(format!("({:6.3}, {:6.3})", raw_x, raw_y))
                .color(egui::Color32::from_rgb(150, 200, 255))
                .family(egui::FontFamily::Monospace),
        );
        ui.label(
            egui::RichText::new(format!("mag={:.3}", raw_magnitude))
                .color(magnitude_color(raw_magnitude))
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
        ui.label(
            egui::RichText::new(format!("mag={:.3}", norm_magnitude))
                .color(magnitude_color(norm_magnitude))
                .family(egui::FontFamily::Monospace),
        );
    });

    // Delta display (difference between raw and normalized)
    let delta_x = stick.normalized_x - raw_x;
    let delta_y = stick.normalized_y - raw_y;
    let delta_mag = norm_magnitude - raw_magnitude;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Delta:").color(egui::Color32::from_gray(140)));
        ui.label(
            egui::RichText::new(format!("({:+.3}, {:+.3})", delta_x, delta_y))
                .color(egui::Color32::from_rgb(255, 200, 100))
                .family(egui::FontFamily::Monospace),
        );
        ui.label(
            egui::RichText::new(format!("dmag={:+.3}", delta_mag))
                .color(egui::Color32::from_rgb(255, 200, 100))
                .family(egui::FontFamily::Monospace),
        );
    });

    ui.add_space(8.0);
}

/// Render a small square plot (for raw gilrs values that form a square)
fn render_square_plot(ui: &mut egui::Ui, x: f32, y: f32, trail_points: &[egui::Pos2]) {
    let size = 100.0;
    let (response, painter) =
        ui.allocate_painter(egui::Vec2::new(size, size), egui::Sense::hover());

    let rect = response.rect;
    let center = rect.center();
    let half = size / 2.0 - 5.0; // Leave margin

    // Square boundary (raw gilrs values form a square -1 to 1)
    let square_rect = egui::Rect::from_center_size(center, egui::vec2(half * 2.0, half * 2.0));
    painter.rect_filled(square_rect, 0.0, egui::Color32::from_gray(30));
    painter.rect_stroke(
        square_rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
    );

    // Draw unit circle inside for reference
    painter.circle_stroke(
        center,
        half,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 80, 120)),
    );

    // Center crosshair
    draw_crosshair(&painter, center, 2.0, egui::Color32::from_gray(60));

    // Draw trail (semi-transparent blue for raw)
    draw_trail(
        &painter,
        center,
        half,
        trail_points,
        egui::Color32::from_rgba_unmultiplied(100, 150, 255, 100),
    );

    // Position marker (blue for raw)
    let pos_x = center.x + x * half;
    let pos_y = center.y - y * half; // Flip Y
    painter.circle_filled(
        egui::pos2(pos_x, pos_y),
        4.0,
        egui::Color32::from_rgb(100, 150, 255),
    );
}

/// Render a small circle plot (for normalized values constrained to unit circle)
fn render_circle_plot(ui: &mut egui::Ui, x: f32, y: f32, trail_points: &[egui::Pos2]) {
    let size = 100.0;
    let (response, painter) =
        ui.allocate_painter(egui::Vec2::new(size, size), egui::Sense::hover());

    let rect = response.rect;
    let center = rect.center();
    let radius = size / 2.0 - 5.0; // Leave margin

    // Circle boundary
    painter.circle_filled(center, radius, egui::Color32::from_gray(30));
    painter.circle_stroke(
        center,
        radius,
        egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
    );

    // Center crosshair
    draw_crosshair(&painter, center, 2.0, egui::Color32::from_gray(60));

    // Draw trail (semi-transparent green for normalized)
    draw_trail(
        &painter,
        center,
        radius,
        trail_points,
        egui::Color32::from_rgba_unmultiplied(100, 255, 100, 100),
    );

    // Position marker (green for normalized)
    let pos_x = center.x + x * radius;
    let pos_y = center.y - y * radius; // Flip Y
    painter.circle_filled(
        egui::pos2(pos_x, pos_y),
        4.0,
        egui::Color32::from_rgb(100, 255, 100),
    );
}

/// Draw a center crosshair on the painter
fn draw_crosshair(painter: &egui::Painter, center: egui::Pos2, size: f32, color: egui::Color32) {
    painter.line_segment(
        [
            egui::pos2(center.x - size, center.y),
            egui::pos2(center.x + size, center.y),
        ],
        egui::Stroke::new(1.0, color),
    );
    painter.line_segment(
        [
            egui::pos2(center.x, center.y - size),
            egui::pos2(center.x, center.y + size),
        ],
        egui::Stroke::new(1.0, color),
    );
}

/// Draw a trail as a polyline from stored points
///
/// Points are stored in normalized coordinates (-1 to 1) and converted to screen space.
fn draw_trail(
    painter: &egui::Painter,
    center: egui::Pos2,
    scale: f32,
    points: &[egui::Pos2],
    color: egui::Color32,
) {
    if points.len() < 2 {
        return;
    }

    // Convert normalized points to screen coordinates
    let screen_points: Vec<egui::Pos2> = points
        .iter()
        .map(|p| egui::pos2(center.x + p.x * scale, center.y - p.y * scale))
        .collect();

    // Draw as a polyline
    painter.add(egui::Shape::line(
        screen_points,
        egui::Stroke::new(1.5, color),
    ));
}

/// Draw a position marker (crosshair + dot) at normalized coordinates
fn draw_position_marker(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    norm_x: f32,
    norm_y: f32,
    color: egui::Color32,
) {
    let pos_x = center.x + norm_x * radius;
    let pos_y = center.y - norm_y * radius; // Flip Y (screen coords)

    // Position crosshair
    let cross_size = 6.0;
    painter.line_segment(
        [
            egui::pos2(pos_x - cross_size, pos_y),
            egui::pos2(pos_x + cross_size, pos_y),
        ],
        egui::Stroke::new(2.0, color),
    );
    painter.line_segment(
        [
            egui::pos2(pos_x, pos_y - cross_size),
            egui::pos2(pos_x, pos_y + cross_size),
        ],
        egui::Stroke::new(2.0, color),
    );

    // Position indicator dot
    painter.circle_filled(egui::pos2(pos_x, pos_y), 3.0, color);
}

/// Get color based on magnitude (green near 1.0, yellow otherwise, gray when small)
fn magnitude_color(magnitude: f32) -> egui::Color32 {
    if magnitude > 0.95 {
        egui::Color32::from_rgb(100, 255, 100)
    } else if magnitude > 0.1 {
        egui::Color32::from_rgb(255, 200, 100)
    } else {
        egui::Color32::from_gray(150)
    }
}

/// Render normalized values text
fn render_normalized_values(ui: &mut egui::Ui, stick: &StickState) {
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
}

/// Render magnitude with color coding
fn render_magnitude(ui: &mut egui::Ui, norm_x: f32, norm_y: f32) {
    let magnitude = (norm_x * norm_x + norm_y * norm_y).sqrt();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Mag: ").color(egui::Color32::from_gray(180)));

        ui.label(
            egui::RichText::new(format!("{:.3}", magnitude))
                .color(magnitude_color(magnitude))
                .family(egui::FontFamily::Monospace),
        );

        ui.label(
            egui::RichText::new("(should reach ~1.0 at full deflection)")
                .color(egui::Color32::from_gray(120))
                .size(10.0),
        );
    });
}

/// Render trigger visualization as a progress bar with raw/normalized values
pub fn render_trigger(
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

        // Raw value (show N/A if not available)
        match trigger.raw {
            Some(raw) => {
                ui.label(
                    egui::RichText::new(format!("{:3}", raw))
                        .color(egui::Color32::from_rgb(150, 200, 255))
                        .family(egui::FontFamily::Monospace),
                );
            },
            None => {
                ui.label(
                    egui::RichText::new("N/A")
                        .color(egui::Color32::from_gray(100))
                        .family(egui::FontFamily::Monospace),
                );
            },
        }

        // Normalized value
        ui.label(
            egui::RichText::new(format!("{:5.3}", trigger.normalized))
                .color(egui::Color32::from_rgb(150, 255, 150))
                .family(egui::FontFamily::Monospace),
        );
    });
}

/// Render button grid showing all button states
pub fn render_buttons(ui: &mut egui::Ui, controller: &ControllerState) {
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
///
/// For XInput: Shows "XInput User Index: N | Packet: N" in blue
/// For Gilrs: Shows "HID: [Controller Name]" in purple
pub fn render_controller_header(ui: &mut egui::Ui, controller: &ControllerState) {
    ui.horizontal(|ui| {
        match &controller.backend {
            ControllerBackend::XInput {
                user_index,
                packet_number,
            } => {
                // Blue color for XInput
                ui.label(
                    egui::RichText::new(format!("XInput User Index: {}", user_index))
                        .color(egui::Color32::from_rgb(150, 200, 255))
                        .size(12.0),
                );

                ui.label(
                    egui::RichText::new(format!("| Packet: {}", packet_number))
                        .color(egui::Color32::from_gray(150))
                        .size(11.0)
                        .family(egui::FontFamily::Monospace),
                );
            },
            ControllerBackend::Gilrs { name, .. } => {
                // Purple color for Gilrs/HID
                ui.label(
                    egui::RichText::new(format!("HID: {}", name))
                        .color(egui::Color32::from_rgb(200, 150, 255))
                        .size(12.0),
                );
            },
        }

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
