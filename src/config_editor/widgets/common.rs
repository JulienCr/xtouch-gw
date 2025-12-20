//! Common reusable widgets for the config editor
//!
//! Provides validated input widgets for text, numbers, etc.

use crate::config_editor::state::EditorState;

/// Validated text input with error display
///
/// Returns (changed, error_message)
pub fn validated_text_edit(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    error: Option<&String>,
    validator: impl Fn(&str) -> Option<String>,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut new_error = None;

    ui.horizontal(|ui| {
        ui.label(label);

        let response = ui.text_edit_singleline(value);

        if response.changed() {
            changed = true;

            // Validate
            new_error = validator(value);
        }
    });

    // Show error if exists
    if let Some(err) = error {
        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), format!("  ⚠ {}", err));
    }

    (changed, new_error)
}

/// Validated u16 number input
///
/// Returns (changed, error_message)
pub fn validated_u16_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u16,
    error: Option<&String>,
    min: u16,
    max: u16,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut new_error = None;
    let mut temp_value = *value;

    ui.horizontal(|ui| {
        ui.label(label);

        let response = ui.add(egui::DragValue::new(&mut temp_value).range(min..=max));

        if response.changed() {
            *value = temp_value;
            changed = true;

            // Validate range
            if *value < min || *value > max {
                new_error = Some(format!("Must be {}-{}", min, max));
            }
        }
    });

    // Show error if exists
    if let Some(err) = error {
        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), format!("  ⚠ {}", err));
    }

    (changed, new_error)
}

/// Validated u8 number input
///
/// Returns (changed, error_message)
pub fn validated_u8_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u8,
    error: Option<&String>,
    min: u8,
    max: u8,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut new_error = None;
    let mut temp_value = *value as i32;

    ui.horizontal(|ui| {
        ui.label(label);

        let response = ui.add(egui::DragValue::new(&mut temp_value).range(min as i32..=max as i32));

        if response.changed() {
            *value = temp_value as u8;
            changed = true;

            // Validate range
            if *value < min || *value > max {
                new_error = Some(format!("Must be {}-{}", min, max));
            }
        }
    });

    // Show error if exists
    if let Some(err) = error {
        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), format!("  ⚠ {}", err));
    }

    (changed, new_error)
}

/// Validated f32 number input
///
/// Returns (changed, error_message)
pub fn validated_f32_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    error: Option<&String>,
    min: f32,
    max: f32,
) -> (bool, Option<String>) {
    let mut changed = false;
    let mut new_error = None;

    ui.horizontal(|ui| {
        ui.label(label);

        let response = ui.add(
            egui::DragValue::new(value)
                .range(min..=max)
                .speed(0.01)
        );

        if response.changed() {
            changed = true;

            // Validate range
            if *value < min || *value > max {
                new_error = Some(format!("Must be {:.2}-{:.2}", min, max));
            }
        }
    });

    // Show error if exists
    if let Some(err) = error {
        ui.colored_label(egui::Color32::from_rgb(255, 100, 100), format!("  ⚠ {}", err));
    }

    (changed, new_error)
}

/// Password text input (masked)
///
/// Returns true if the value changed
pub fn password_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);

        let response = ui.add(egui::TextEdit::singleline(value).password(true));

        response.changed()
    })
    .inner
}

/// Checkbox input
///
/// Returns true if the value changed
pub fn checkbox_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut bool,
) -> bool {
    let response = ui.checkbox(value, label);

    response.changed()
}
