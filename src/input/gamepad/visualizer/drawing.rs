//! Low-level drawing primitives for gamepad visualizer
//!
//! Contains helper functions for drawing common UI elements like
//! crosshairs, trails, position markers, and plot backgrounds.

use egui::{Color32, Painter, Pos2, Stroke};

/// Draw a center crosshair on the painter
pub fn draw_crosshair(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    painter.line_segment(
        [
            egui::pos2(center.x - size, center.y),
            egui::pos2(center.x + size, center.y),
        ],
        Stroke::new(1.0, color),
    );
    painter.line_segment(
        [
            egui::pos2(center.x, center.y - size),
            egui::pos2(center.x, center.y + size),
        ],
        Stroke::new(1.0, color),
    );
}

/// Draw a trail as a polyline from stored points
///
/// Points are stored in normalized coordinates (-1 to 1) and converted to screen space.
pub fn draw_trail(painter: &Painter, center: Pos2, scale: f32, points: &[Pos2], color: Color32) {
    if points.len() < 2 {
        return;
    }

    // Convert normalized points to screen coordinates
    let screen_points: Vec<Pos2> = points
        .iter()
        .map(|p| egui::pos2(center.x + p.x * scale, center.y - p.y * scale))
        .collect();

    // Draw as a polyline
    painter.add(egui::Shape::line(screen_points, Stroke::new(1.5, color)));
}

/// Draw a position marker (crosshair + dot) at normalized coordinates
pub fn draw_position_marker(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    norm_x: f32,
    norm_y: f32,
    color: Color32,
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
        Stroke::new(2.0, color),
    );
    painter.line_segment(
        [
            egui::pos2(pos_x, pos_y - cross_size),
            egui::pos2(pos_x, pos_y + cross_size),
        ],
        Stroke::new(2.0, color),
    );

    // Position indicator dot
    painter.circle_filled(egui::pos2(pos_x, pos_y), 3.0, color);
}

/// Get color based on magnitude (green near 1.0, yellow otherwise, gray when small)
pub fn magnitude_color(magnitude: f32) -> Color32 {
    if magnitude > 0.95 {
        Color32::from_rgb(100, 255, 100)
    } else if magnitude > 0.1 {
        Color32::from_rgb(255, 200, 100)
    } else {
        Color32::from_gray(150)
    }
}

/// Render a small square plot (for raw gilrs values that form a square)
pub fn render_square_plot(ui: &mut egui::Ui, x: f32, y: f32, trail_points: &[Pos2]) {
    let size = 100.0;
    let (response, painter) =
        ui.allocate_painter(egui::Vec2::new(size, size), egui::Sense::hover());

    let rect = response.rect;
    let center = rect.center();
    let half = size / 2.0 - 5.0; // Leave margin

    // Square boundary (raw gilrs values form a square -1 to 1)
    let square_rect = egui::Rect::from_center_size(center, egui::vec2(half * 2.0, half * 2.0));
    painter.rect_filled(square_rect, 0.0, Color32::from_gray(30));
    painter.rect_stroke(square_rect, 0.0, Stroke::new(1.0, Color32::from_gray(100)));

    // Draw unit circle inside for reference
    painter.circle_stroke(
        center,
        half,
        Stroke::new(1.0, Color32::from_rgb(80, 80, 120)),
    );

    // Center crosshair
    draw_crosshair(&painter, center, 2.0, Color32::from_gray(60));

    // Draw trail (semi-transparent blue for raw)
    draw_trail(
        &painter,
        center,
        half,
        trail_points,
        Color32::from_rgba_unmultiplied(100, 150, 255, 100),
    );

    // Position marker (blue for raw)
    let pos_x = center.x + x * half;
    let pos_y = center.y - y * half; // Flip Y
    painter.circle_filled(
        egui::pos2(pos_x, pos_y),
        4.0,
        Color32::from_rgb(100, 150, 255),
    );
}

/// Render a small circle plot (for normalized values constrained to unit circle)
pub fn render_circle_plot(ui: &mut egui::Ui, x: f32, y: f32, trail_points: &[Pos2]) {
    let size = 100.0;
    let (response, painter) =
        ui.allocate_painter(egui::Vec2::new(size, size), egui::Sense::hover());

    let rect = response.rect;
    let center = rect.center();
    let radius = size / 2.0 - 5.0; // Leave margin

    // Circle boundary
    painter.circle_filled(center, radius, Color32::from_gray(30));
    painter.circle_stroke(center, radius, Stroke::new(1.0, Color32::from_gray(100)));

    // Center crosshair
    draw_crosshair(&painter, center, 2.0, Color32::from_gray(60));

    // Draw trail (semi-transparent green for normalized)
    draw_trail(
        &painter,
        center,
        radius,
        trail_points,
        Color32::from_rgba_unmultiplied(100, 255, 100, 100),
    );

    // Position marker (green for normalized)
    let pos_x = center.x + x * radius;
    let pos_y = center.y - y * radius; // Flip Y
    painter.circle_filled(
        egui::pos2(pos_x, pos_y),
        4.0,
        Color32::from_rgb(100, 255, 100),
    );
}
