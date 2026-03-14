//! X-Touch display update helpers.
//!
//! Centralizes the logic for updating LCD labels, colors, F-key LEDs, and
//! prev/next navigation LEDs on the X-Touch hardware. This logic is reused
//! during initial setup, page changes, and configuration reloads.

use std::sync::Arc;

use tracing::warn;

use crate::config::PageConfig;
use crate::midi::MidiMessage;
use crate::router::Router;
use crate::xtouch::XTouchDriver;

/// Update the X-Touch display for the currently active page.
///
/// This applies LCD labels and colors, F-key LEDs (page indicators), and
/// prev/next navigation LEDs. It reads the active page and paging config
/// from the router at call time, so it always reflects the latest state.
pub async fn update_xtouch_display(router: &Router, xtouch: &Arc<XTouchDriver>) {
    // Read config once to extract all needed fields
    let (active_page, active_page_name, paging_channel, paging) = {
        let config = router.config.read().await;
        let index = *router.active_page_index.read().await;
        let page = config.pages.get(index).cloned();
        let name = page
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "(none)".to_string());
        let paging_channel = config.paging.as_ref().map(|p| p.channel).unwrap_or(1);
        let paging = config.paging.clone();
        (page, name, paging_channel, paging)
    };

    if let Some(page) = &active_page {
        let labels = page.lcd.as_ref().and_then(|lcd| lcd.labels.as_ref());
        let colors_u8 = convert_lcd_colors(page);

        if let Err(e) = xtouch
            .apply_lcd_for_page(labels, colors_u8.as_ref(), &active_page_name)
            .await
        {
            warn!("Failed to apply LCD for page: {}", e);
        }
    }

    if let Err(e) = router
        .update_fkey_leds_for_active_page(xtouch, paging_channel)
        .await
    {
        warn!("Failed to update F-key LEDs: {}", e);
    }

    // Update prev/next navigation LEDs (always on)
    if let Some(paging) = &paging {
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

/// Flush pending MIDI messages from the router to the X-Touch hardware.
///
/// Takes all queued messages (e.g., from page refresh) and sends them sequentially.
/// Used after page changes, config reloads, and startup refresh.
pub async fn flush_pending_midi(router: &Router, xtouch: &Arc<XTouchDriver>, label: &str) {
    let pending = router.take_pending_midi().await;
    for msg in pending {
        tracing::trace!("  -> Sending {} MIDI: {:02X?}", label, msg);
        if let Err(e) = xtouch.send_raw(&msg).await {
            warn!("Failed to send {} MIDI: {}", label, e);
        }
    }
}

/// Extract PitchBend channel and 14-bit value from raw MIDI feedback data.
///
/// This helper is used to detect PitchBend messages early in the feedback handling
/// path so that squelch can be activated BEFORE state updates (BUG-002 fix).
///
/// Returns `Some((channel, value14))` if the data is a valid PitchBend message,
/// or `None` for all other message types.
pub fn extract_pitchbend_from_feedback(data: &[u8]) -> Option<(u8, u16)> {
    match MidiMessage::parse(data)? {
        MidiMessage::PitchBend { channel, value } => Some((channel, value)),
        _ => None,
    }
}
