//! Page refresh logic and state replay

use crate::config::{ControlMapping, PageConfig};
use crate::control_mapping::{ControlMappingDB, MidiSpec};
use crate::state::{MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin};
use tracing::{debug, info, trace};

impl super::Router {
    /// Refresh the active page (replay all known states to X-Touch)
    pub async fn refresh_page(&self) {
        // BUG-006 FIX: Increment epoch FIRST to invalidate any in-flight feedback
        // This must happen BEFORE clearing shadow state to prevent race conditions
        // where feedback arrives after shadow clear but before refresh completes.
        let new_epoch = self.increment_page_epoch();

        // BUG-009 FIX: Update fader setpoint's page epoch to invalidate stale setpoints
        // This must happen AFTER incrementing page_epoch so that old setpoints are
        // rejected when plan_page_refresh() calls get_desired()
        self.fader_setpoint.set_page_epoch(new_epoch);

        let page = match self.get_active_page().await {
            Some(p) => p,
            None => return,
        };

        debug!("Refreshing page '{}' (epoch={})", page.name, new_epoch);

        // Clear X-Touch shadow state to allow re-emission (fire-and-forget)
        self.state_actor.clear_shadows();

        // Build and execute refresh plan
        let entries = self.plan_page_refresh(&page).await;

        trace!(
            "Page refresh plan: {} Notes, {} CCs, {} PBs",
            entries
                .iter()
                .filter(|e| e.addr.status == MidiStatus::Note)
                .count(),
            entries
                .iter()
                .filter(|e| e.addr.status == MidiStatus::CC)
                .count(),
            entries
                .iter()
                .filter(|e| e.addr.status == MidiStatus::PB)
                .count()
        );

        // Log PB entries in detail
        for entry in entries.iter().filter(|e| e.addr.status == MidiStatus::PB) {
            trace!(
                "  PB entry: ch={} value={:?} port={}",
                entry.addr.channel.unwrap_or(0),
                entry.value,
                entry.addr.port_id
            );
        }

        // Convert entries to MIDI bytes and collect for sending
        let mut midi_messages = Vec::new();
        for entry in &entries {
            let bytes = self.entry_to_midi_bytes(entry);
            if !bytes.is_empty() {
                if entry.addr.status == MidiStatus::PB {
                    trace!(
                        "  Converting PB entry to MIDI: ch={} value={:?} → bytes={:02X?}",
                        entry.addr.channel.unwrap_or(0),
                        entry.value,
                        bytes
                    );
                }
                midi_messages.push(bytes);
            }
        }

        info!(
            "Page refresh completed: {} (sending {} MIDI messages)",
            page.name,
            midi_messages.len()
        );

        // Store pending MIDI messages for main loop to send to X-Touch
        *self.pending_midi_messages.lock().await = midi_messages;

        // Signal that display needs update (LCD + LEDs)
        *self.display_needs_update.lock().await = true;
    }

    /// Check if a specific app is mapped to a fader channel on the given page
    ///
    /// BUG-010 FIX: Used during page refresh to prevent PB state from one app
    /// (e.g., voicemeeter on page 1) from overriding the CC→PB transform of
    /// another app (e.g., qlc on page 2) for the same fader channel.
    pub(super) fn is_app_mapped_to_fader(
        page: &PageConfig,
        global_config: &crate::config::AppConfig,
        app_name: &str,
        pb_channel: u8,
        mapping_db: &ControlMappingDB,
    ) -> bool {
        // PB channels are 1-based (faders 1-9), MIDI spec uses 0-based
        if pb_channel == 0 {
            return false;
        }
        let control_id = Self::find_control_by_midi_spec(
            mapping_db,
            |spec| matches!(spec, MidiSpec::PitchBend { channel } if *channel == pb_channel - 1),
        );

        let Some(control_id) = control_id else {
            return false;
        };

        match Self::get_control_config(page, global_config, &control_id) {
            Some(config) => config.app == app_name,
            None => false,
        }
    }

    /// Convert MidiStateEntry to raw MIDI bytes for sending to X-Touch
    pub(crate) fn entry_to_midi_bytes(&self, entry: &MidiStateEntry) -> Vec<u8> {
        // Convert external channel (1-16) to MIDI wire format (0-15)
        let channel = entry
            .addr
            .channel
            .map(|ch| ch.saturating_sub(1))
            .unwrap_or(0);
        let data1 = entry.addr.data1.unwrap_or(0);

        match entry.addr.status {
            MidiStatus::Note => {
                let velocity = match &entry.value {
                    MidiValue::Number(v) => (*v as u8).min(127),
                    _ => 0,
                };
                vec![0x90 | channel, data1, velocity]
            },
            MidiStatus::CC => {
                let value = match &entry.value {
                    MidiValue::Number(v) => (*v as u8).min(127),
                    _ => 0,
                };
                vec![0xB0 | channel, data1, value]
            },
            MidiStatus::PB => {
                let value14 = match &entry.value {
                    MidiValue::Number(v) => (*v).min(16383),
                    _ => 0,
                };
                let lsb = (value14 & 0x7F) as u8;
                let msb = ((value14 >> 7) & 0x7F) as u8;
                vec![0xE0 | channel, lsb, msb]
            },
            _ => vec![],
        }
    }

    /// Find control ID by matching a MIDI spec predicate
    pub(super) fn find_control_by_midi_spec<F>(
        mapping_db: &ControlMappingDB,
        predicate: F,
    ) -> Option<String>
    where
        F: Fn(&MidiSpec) -> bool,
    {
        mapping_db
            .mappings
            .iter()
            .find(|(_, mapping)| {
                MidiSpec::parse(&mapping.mcu_message)
                    .map(|spec| predicate(&spec))
                    .unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
    }

    /// Look up control config from page controls, falling back to global controls
    pub(super) fn get_control_config(
        page: &PageConfig,
        global_config: &crate::config::AppConfig,
        control_id: &str,
    ) -> Option<ControlMapping> {
        // Try page controls first
        if let Some(config) = page.controls.as_ref().and_then(|c| c.get(control_id)) {
            return Some(config.clone());
        }

        // Fall back to global controls
        global_config
            .pages_global
            .as_ref()
            .and_then(|g| g.controls.as_ref())
            .and_then(|c| c.get(control_id))
            .cloned()
    }

    /// Create a reset MidiStateEntry (value=0) for page refresh
    pub(super) fn make_reset_entry(
        port_id: &str,
        status: MidiStatus,
        channel: u8,
        data1: u8,
    ) -> MidiStateEntry {
        MidiStateEntry {
            addr: MidiAddr {
                port_id: port_id.to_string(),
                status,
                channel: Some(channel),
                data1: Some(data1),
            },
            value: MidiValue::Number(0),
            ts: Self::now_ms(),
            origin: Origin::XTouch,
            known: false,
            stale: false,
            hash: None,
        }
    }
}
