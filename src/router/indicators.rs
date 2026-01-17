//! LED indicator evaluation and F-key management

use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use tracing::debug;

impl super::Router {
    /// Evaluate indicator conditions for a signal emission
    ///
    /// Returns a HashMap of control_id -> should_be_lit for all controls
    /// on the active page that have indicators matching the given signal.
    ///
    /// This is called by the indicator subscription handler when drivers
    /// emit signals (e.g., "obs.selectedScene", "obs.studioMode").
    pub async fn evaluate_indicators(&self, signal: &str, value: &Value) -> HashMap<String, bool> {
        let mut result = HashMap::new();

        // Get active page controls
        let config = self.config.read().await;
        let page_index = *self.active_page_index.read().await;

        let page = match config.pages.get(page_index) {
            Some(p) => p,
            None => return result,
        };

        let controls = match &page.controls {
            Some(c) => c,
            None => return result,
        };

        // Also check global controls
        let global_controls = config
            .pages_global
            .as_ref()
            .and_then(|g| g.controls.as_ref());

        // Iterate through all controls (page + global)
        // First check page controls
        for (control_id, mapping) in controls.iter() {
            let indicator = match &mapping.indicator {
                Some(ind) => ind,
                None => continue,
            };

            // Check if this indicator matches the signal
            if indicator.signal != signal {
                continue;
            }

            // Evaluate the condition
            let should_be_lit = self.evaluate_indicator_condition(indicator, value);
            result.insert(control_id.clone(), should_be_lit);
        }

        // Then check global controls
        if let Some(global_ctrls) = global_controls {
            for (control_id, mapping) in global_ctrls.iter() {
                let indicator = match &mapping.indicator {
                    Some(ind) => ind,
                    None => continue,
                };

                // Check if this indicator matches the signal
                if indicator.signal != signal {
                    continue;
                }

                // Evaluate the condition
                let should_be_lit = self.evaluate_indicator_condition(indicator, value);
                result.insert(control_id.clone(), should_be_lit);
            }
        }

        result
    }

    /// Helper to evaluate a single indicator condition
    fn evaluate_indicator_condition(
        &self,
        indicator: &crate::config::IndicatorConfig,
        value: &Value,
    ) -> bool {
        if let Some(truthy) = indicator.truthy {
            // Truthy check: LED on if value is truthy
            if truthy {
                match value {
                    Value::Bool(b) => *b,
                    Value::Null => false,
                    Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
                    Value::String(s) => !s.is_empty(),
                    Value::Array(a) => !a.is_empty(),
                    Value::Object(o) => !o.is_empty(),
                }
            } else {
                false
            }
        } else if let Some(in_array) = &indicator.in_array {
            // "in" check: LED on if value matches any in array
            in_array.iter().any(|v| {
                // String comparison: trim and compare
                if let (Value::String(a), Value::String(b)) = (v, value) {
                    a.trim() == b.trim()
                } else {
                    // Use serde_json equality (similar to Object.is)
                    *v == *value
                }
            })
        } else if let Some(equals_value) = &indicator.equals {
            // Equals check: LED on if value matches exactly
            if let (Value::String(a), Value::String(b)) = (equals_value, value) {
                a.trim() == b.trim()
            } else {
                *equals_value == *value
            }
        } else {
            // No condition specified, default to off
            false
        }
    }

    /// Update F1-F8 LEDs to reflect active page
    ///
    /// Matches TypeScript updateFKeyLedsForActivePage() from xtouch/fkeys.ts
    pub async fn update_fkey_leds_for_active_page(
        &self,
        xtouch: &crate::xtouch::XTouchDriver,
        _paging_channel: u8,
    ) -> Result<()> {
        let config = self.config.read().await;
        let active_index = *self.active_page_index.read().await;

        // Get F-key notes based on mode
        let mode = config
            .xtouch
            .as_ref()
            .map(|x| x.mode)
            .unwrap_or(crate::config::XTouchMode::Mcu);
        let fkey_notes = self.get_fkey_notes(mode);

        // Clamp active index to valid range
        let clamped_index = if active_index < fkey_notes.len() {
            active_index as i32
        } else {
            (fkey_notes.len().saturating_sub(1)) as i32
        };

        // Update LEDs - ALWAYS turn all off first, then light the active one
        for (i, &note) in fkey_notes.iter().enumerate() {
            let on = (i as i32) == clamped_index;
            xtouch.set_button_led(note, on).await?;
        }

        debug!(
            "F-key LEDs updated: active index {} (note {})",
            clamped_index,
            fkey_notes.get(clamped_index as usize).copied().unwrap_or(0)
        );

        Ok(())
    }

    /// Update prev/next navigation button LEDs (always on)
    ///
    /// Matches TypeScript updatePrevNextLeds() from xtouch/fkeys.ts
    pub async fn update_prev_next_leds(
        &self,
        xtouch: &crate::xtouch::XTouchDriver,
        prev_note: u8,
        next_note: u8,
    ) -> Result<()> {
        xtouch.set_button_led(prev_note, true).await?;
        xtouch.set_button_led(next_note, true).await?;
        Ok(())
    }

    /// Get F-key note numbers based on X-Touch mode
    fn get_fkey_notes(&self, mode: crate::config::XTouchMode) -> Vec<u8> {
        // From xtouch-matching.csv for MCU mode:
        // f1 = 54, f2 = 55, f3 = 56, f4 = 57, f5 = 58, f6 = 59, f7 = 60, f8 = 61
        // These are the default note numbers for F1-F8 in both MCU and Ctrl modes
        match mode {
            crate::config::XTouchMode::Mcu | crate::config::XTouchMode::Ctrl => {
                vec![54, 55, 56, 57, 58, 59, 60, 61]
            },
        }
    }
}
