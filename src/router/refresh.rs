//! Page refresh logic and state replay

use crate::config::{ControlMapping, PageConfig};
use crate::control_mapping::{load_default_mappings, ControlMappingDB, MidiSpec};
use crate::state::{AppKey, MidiAddr, MidiStateEntry, MidiStatus, MidiValue, Origin};
use std::collections::HashMap;
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
                // Always use Note On - velocity 0 turns LED off, velocity >0 turns it on
                // NoteOff (0x80) is for button release events, not LED control
                vec![0x90 | channel, data1, velocity]
            },
            MidiStatus::CC => {
                let value = match &entry.value {
                    MidiValue::Number(v) => (*v as u8).min(127),
                    _ => 0,
                };
                vec![0xB0 | channel, data1, value] // Control Change
            },
            MidiStatus::PB => {
                let value14 = match &entry.value {
                    MidiValue::Number(v) => (*v).min(16383),
                    _ => 0,
                };
                let lsb = (value14 & 0x7F) as u8;
                let msb = ((value14 >> 7) & 0x7F) as u8;
                vec![0xE0 | channel, lsb, msb] // Pitch Bend
            },
            _ => vec![], // Other types not handled
        }
    }

    /// Find control ID by matching a MIDI spec predicate
    fn find_control_by_midi_spec<F>(mapping_db: &ControlMappingDB, predicate: F) -> Option<String>
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
    fn get_control_config(&self, page: &PageConfig, control_id: &str) -> Option<ControlMapping> {
        // Try page controls first
        if let Some(config) = page.controls.as_ref().and_then(|c| c.get(control_id)) {
            return Some(config.clone());
        }

        // Fall back to global controls
        let config_guard = self.config.try_read().expect("Config lock poisoned");
        config_guard
            .pages_global
            .as_ref()
            .and_then(|g| g.controls.as_ref())
            .and_then(|c| c.get(control_id))
            .cloned()
    }

    /// Query CC value from StateActor for a control mapping
    async fn get_cc_value_for_control(
        &self,
        app: &AppKey,
        control_config: &ControlMapping,
    ) -> Option<(MidiStateEntry, u8)> {
        let midi_spec = control_config.midi.as_ref()?;

        if !matches!(midi_spec.midi_type, crate::config::MidiType::Cc) {
            return None;
        }

        let cc_channel = midi_spec.channel?;
        let cc_num = midi_spec.cc?;

        let cc_entry = self
            .state_actor
            .get_known_latest(*app, MidiStatus::CC, Some(cc_channel), Some(cc_num))
            .await?;

        let cc_value = cc_entry.value.as_number()? as u8;
        Some((cc_entry, cc_value))
    }

    /// Try to transform CC value to PB for page refresh (reverse transformation)
    ///
    /// When a fader is mapped to send CC to an app (like QLC+), the StateStore
    /// will have CC values. But X-Touch faders need PB messages. This function
    /// looks up the control mapping and transforms CC (7-bit) to PB (14-bit).
    async fn try_cc_to_pb_transform(
        &self,
        page: &PageConfig,
        app: &AppKey,
        pb_channel: u8,
    ) -> Option<MidiStateEntry> {
        let mapping_db = load_default_mappings().ok()?;

        // Find control ID for this PB channel (e.g., "fader1" for ch1)
        let control_id = Self::find_control_by_midi_spec(
            &mapping_db,
            |spec| matches!(spec, MidiSpec::PitchBend { channel } if *channel == pb_channel.saturating_sub(1)),
        )?;

        trace!(
            "CC->PB transform: control={} pb_channel={}",
            control_id,
            pb_channel
        );

        let control_config = self.get_control_config(page, &control_id)?;

        // Ensure control's app matches
        if control_config.app != app.as_str() {
            return None;
        }

        let (cc_entry, cc_value) = self.get_cc_value_for_control(app, &control_config).await?;
        let pb_value = crate::midi::convert::to_14bit(cc_value);

        trace!(
            "  Transform CC {} -> PB {} (0x{:04X})",
            cc_value,
            pb_value,
            pb_value
        );

        Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: app.as_str().to_string(),
                status: MidiStatus::PB,
                channel: Some(pb_channel),
                data1: Some(0),
            },
            value: MidiValue::Number(pb_value),
            ts: cc_entry.ts,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        })
    }

    /// Try to transform CC value to Note for page refresh (reverse transformation)
    ///
    /// When a button is mapped to send CC to an app (like QLC+), the StateStore
    /// will have CC values. But X-Touch buttons need Note messages.
    async fn try_cc_to_note_transform(
        &self,
        page: &PageConfig,
        app: &AppKey,
        note: u8,
    ) -> Option<MidiStateEntry> {
        let mapping_db = load_default_mappings().ok()?;

        // Find control ID for this Note (e.g., "mute1" for note=16)
        let control_id = Self::find_control_by_midi_spec(
            &mapping_db,
            |spec| matches!(spec, MidiSpec::Note { note: n } if *n == note),
        )?;

        let control_config = self.get_control_config(page, &control_id)?;

        // Ensure control's app matches
        if control_config.app != app.as_str() {
            return None;
        }

        let (cc_entry, cc_value) = self.get_cc_value_for_control(app, &control_config).await?;
        let velocity = if cc_value > 0 { 127 } else { 0 };

        trace!(
            "CC->Note transform: {} CC {} -> Note {} velocity {}",
            control_id,
            cc_value,
            note,
            velocity
        );

        Some(MidiStateEntry {
            addr: MidiAddr {
                port_id: app.as_str().to_string(),
                status: MidiStatus::Note,
                channel: Some(1), // X-Touch buttons are on channel 1
                data1: Some(note),
            },
            value: MidiValue::Number(velocity as u16),
            ts: cc_entry.ts,
            origin: Origin::App,
            known: true,
            stale: false,
            hash: None,
        })
    }

    /// Create a reset MidiStateEntry (value=0) for page refresh
    fn make_reset_entry(
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

    /// Plan page refresh: build ordered list of MIDI entries to send
    ///
    /// Returns entries in order: Notes -> CC -> SysEx -> PB
    /// Priority for each type:
    /// - PB: Known PB = 3 > Mapped CC = 2 > Zero = 1
    /// - Notes/CC: Known value = 2 > Reset (0/OFF) = 1
    async fn plan_page_refresh(&self, _page: &PageConfig) -> Vec<MidiStateEntry> {
        #[derive(Clone)]
        struct PlanEntry {
            entry: MidiStateEntry,
            priority: u8,
        }

        /// Insert entry with priority-based replacement (higher priority wins, then newer timestamp)
        fn insert_prioritized<K: std::hash::Hash + Eq>(
            map: &mut HashMap<K, PlanEntry>,
            key: K,
            entry: MidiStateEntry,
            priority: u8,
        ) {
            let dominated = map.get(&key).is_none_or(|cur| {
                priority > cur.priority || (priority == cur.priority && entry.ts > cur.entry.ts)
            });
            if dominated {
                map.insert(key, PlanEntry { entry, priority });
            }
        }

        let mut note_plan: HashMap<String, PlanEntry> = HashMap::new();
        let mut cc_plan: HashMap<String, PlanEntry> = HashMap::new();
        let mut pb_plan: HashMap<u8, PlanEntry> = HashMap::new();

        /// Generate channel|data1 key for Note/CC plans
        fn channel_data1_key(entry: &MidiStateEntry) -> String {
            format!(
                "{}|{}",
                entry.addr.channel.unwrap_or(0),
                entry.addr.data1.unwrap_or(0)
            )
        }

        // X-Touch buttons are on channel 1 (0-indexed as channel 0 in MIDI, but config uses 1)
        // Faders use channels 1-9 for PitchBend: 8 strip faders + 1 master fader
        // (fader1-8 on ch1-8, fader_master on ch9)
        let channels: Vec<u8> = (1..=9).collect();

        // Get apps mapped on this page (only restore state for mapped apps)
        let config = self.config.try_read().expect("Config lock poisoned");
        let apps_on_page = self.get_apps_for_page(_page, &config);

        // Build plans for each app (only apps mapped on this page)
        for app in AppKey::all() {
            // Skip apps not mapped on this page
            if !apps_on_page.contains(app.as_str()) {
                continue;
            }

            // PB plan (priority: Known PB = 3 > Mapped CC->PB = 2)
            for &ch in &channels {
                if let Some(latest_pb) = self
                    .state_actor
                    .get_known_latest(*app, MidiStatus::PB, Some(ch), Some(0))
                    .await
                {
                    insert_prioritized(&mut pb_plan, ch, latest_pb, 3);
                    continue;
                }

                if let Some(transformed_pb) = self.try_cc_to_pb_transform(_page, app, ch).await {
                    trace!(
                        "  Adding CC->PB to plan: ch={} value={:?}",
                        ch,
                        transformed_pb.value
                    );
                    insert_prioritized(&mut pb_plan, ch, transformed_pb, 2);
                }
            }

            // Notes: 0-31 - Known state from CC transform (priority 2) or Note Off (priority 1)
            for note in 0..=31 {
                if let Some(transformed_note) =
                    self.try_cc_to_note_transform(_page, app, note).await
                {
                    let key = channel_data1_key(&transformed_note);
                    insert_prioritized(&mut note_plan, key, transformed_note, 2);
                    continue;
                }

                let off = Self::make_reset_entry(app.as_str(), MidiStatus::Note, 1, note);
                let key = channel_data1_key(&off);
                insert_prioritized(&mut note_plan, key, off, 1);
            }

            // CC (rings): 0-31 - Always send 0 to clear previous page
            for &ch in &channels {
                for cc in 0..=31 {
                    let zero = Self::make_reset_entry(app.as_str(), MidiStatus::CC, ch, cc);
                    let key = channel_data1_key(&zero);
                    insert_prioritized(&mut cc_plan, key, zero, 1);
                }
            }
        }

        // Fill missing fader channels with setpoint or zero fallback
        for &ch in &channels {
            if pb_plan.contains_key(&ch) {
                continue;
            }

            if let Some(desired14) = self.fader_setpoint.get_desired(ch) {
                let setpoint_pb = MidiStateEntry {
                    addr: MidiAddr {
                        port_id: "xtouch".to_string(),
                        status: MidiStatus::PB,
                        channel: Some(ch),
                        data1: Some(0),
                    },
                    value: MidiValue::Number(desired14),
                    ts: Self::now_ms(),
                    origin: Origin::XTouch,
                    known: false,
                    stale: false,
                    hash: None,
                };
                insert_prioritized(&mut pb_plan, ch, setpoint_pb, 2);
                continue;
            }

            let zero_pb = Self::make_reset_entry("xtouch", MidiStatus::PB, ch, 0);
            insert_prioritized(&mut pb_plan, ch, zero_pb, 1);
        }

        // Materialize plans into ordered list: Notes → CC → PB
        let mut entries = Vec::new();

        // Notes first
        for plan_entry in note_plan.values() {
            entries.push(plan_entry.entry.clone());
        }

        // Then CC
        for plan_entry in cc_plan.values() {
            entries.push(plan_entry.entry.clone());
        }

        // Finally PB
        for plan_entry in pb_plan.values() {
            entries.push(plan_entry.entry.clone());
        }

        entries
    }
}
