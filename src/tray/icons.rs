//! Icon generation for system tray
//!
//! Generates simple programmatic icons for different connection states.

use image::{ImageBuffer, Rgba};

/// Generate a 16x16 icon with a colored circle
///
/// Colors represent connection states:
/// - Green: Connected
/// - Red: Disconnected
/// - Yellow: Reconnecting
pub fn generate_icon(color: IconColor) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let mut img = ImageBuffer::new(16, 16);

    let (r, g, b) = match color {
        IconColor::Green => (0, 200, 0),
        IconColor::Red => (200, 0, 0),
        IconColor::Yellow => (200, 200, 0),
        IconColor::Gray => (128, 128, 128),
    };

    // Draw a filled circle in the center
    let center_x = 8.0;
    let center_y = 8.0;
    let radius = 6.0;

    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let dx = x as f32 - center_x;
        let dy = y as f32 - center_y;
        let distance = (dx * dx + dy * dy).sqrt();

        if distance <= radius {
            *pixel = Rgba([r, g, b, 255]);
        } else {
            *pixel = Rgba([0, 0, 0, 0]); // Transparent background
        }
    }

    img
}

/// Icon colors for different states
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IconColor {
    Green,
    Red,
    Yellow,
    Gray,
}

/// Convert ImageBuffer to RGBA bytes for tray-icon
pub fn to_rgba_bytes(img: &ImageBuffer<Rgba<u8>, Vec<u8>>) -> Vec<u8> {
    img.as_raw().clone()
}

/// Generate icon and return as RGBA bytes
pub fn generate_icon_bytes(color: IconColor) -> Vec<u8> {
    let img = generate_icon(color);
    to_rgba_bytes(&img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_icon() {
        let img = generate_icon(IconColor::Green);
        assert_eq!(img.width(), 16);
        assert_eq!(img.height(), 16);
    }

    #[test]
    fn test_icon_colors() {
        let green = generate_icon(IconColor::Green);
        let red = generate_icon(IconColor::Red);

        // Center pixel should be colored
        assert_eq!(green.get_pixel(8, 8)[1], 200); // Green channel
        assert_eq!(red.get_pixel(8, 8)[0], 200); // Red channel
    }

    #[test]
    fn test_to_rgba_bytes() {
        let img = generate_icon(IconColor::Green);
        let bytes = to_rgba_bytes(&img);

        // 16x16 pixels * 4 bytes per pixel (RGBA)
        assert_eq!(bytes.len(), 16 * 16 * 4);
    }
}
