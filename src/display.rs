//! X-Touch display update helpers.
//!
//! Centralizes the logic for updating LCD labels, colors, F-key LEDs, and
//! prev/next navigation LEDs on the X-Touch hardware. This logic is reused
//! during initial setup, page changes, and configuration reloads.

use std::sync::Arc;

use tracing::warn;

use crate::config::PageConfig;
use crate::router::Router;
use crate::xtouch::XTouchDriver;

/// Update the X-Touch display for the currently active page.
///
/// This applies LCD labels and colors, F-key LEDs (page indicators), and
/// prev/next navigation LEDs. It reads the active page and paging config
/// from the router at call time, so it always reflects the latest state.
pub async fn update_xtouch_display(router: &Router, xtouch: &Arc<XTouchDriver>) {
    // Get active page config
    let active_page = router.get_active_page().await;
    let active_page_name = router.get_active_page_name().await;

    if let Some(page) = active_page {
        let labels = page.lcd.as_ref().and_then(|lcd| lcd.labels.as_ref());
        let colors_u8 = convert_lcd_colors(&page);

        if let Err(e) = xtouch
            .apply_lcd_for_page(labels, colors_u8.as_ref(), &active_page_name)
            .await
        {
            warn!("Failed to apply LCD for page: {}", e);
        }
    }

    // Update F-key LEDs to show active page
    let router_config = router.config.read().await;
    let paging_channel = router_config
        .paging
        .as_ref()
        .map(|p| p.channel)
        .unwrap_or(1);
    let paging_clone = router_config.paging.clone();
    drop(router_config);

    if let Err(e) = router
        .update_fkey_leds_for_active_page(xtouch, paging_channel)
        .await
    {
        warn!("Failed to update F-key LEDs: {}", e);
    }

    // Update prev/next navigation LEDs (always on)
    if let Some(paging) = &paging_clone {
        if let Err(e) = router
            .update_prev_next_leds(xtouch, paging.prev_note, paging.next_note)
            .await
        {
            warn!("Failed to update prev/next LEDs: {}", e);
        }
    }
}

/// Convert LCD colors from a page config to u8 values.
pub fn convert_lcd_colors(page: &PageConfig) -> Option<Vec<u8>> {
    page.lcd.as_ref().and_then(|lcd| {
        lcd.colors
            .as_ref()
            .map(|colors| colors.iter().map(|c| c.to_u8()).collect())
    })
}

/// Extract PitchBend channel and 14-bit value from raw MIDI feedback data.
///
/// This helper is used to detect PitchBend messages early in the feedback handling
/// path so that squelch can be activated BEFORE state updates (BUG-002 fix).
///
/// Returns `Some((channel, value14))` if the data is a valid PitchBend message,
/// or `None` for all other message types.
pub fn extract_pitchbend_from_feedback(data: &[u8]) -> Option<(u8, u16)> {
    // PitchBend message format: [0xE0-0xEF, LSB, MSB]
    // Status byte: 0xEn where n is the channel (0-15)
    if data.len() >= 3 {
        let status = data[0];
        if (status & 0xF0) == 0xE0 {
            let channel = status & 0x0F;
            let lsb = data[1] & 0x7F; // 7-bit LSB
            let msb = data[2] & 0x7F; // 7-bit MSB
            let value14 = ((msb as u16) << 7) | (lsb as u16);
            return Some((channel, value14));
        }
    }
    None
}
