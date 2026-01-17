//! Page refresh logic and state replay

use crate::config::PageConfig;
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

    /// Try to transform CC value to PB for page refresh (reverse transformation)
    ///
    /// When a fader is mapped to send CC to an app (like QLC+), the StateStore
    /// will have CC values. But X-Touch faders need PB messages. This function:
    /// 1. Looks up which control uses the given PB channel
    /// 2. Checks if that control has a CC mapping in the page config
    /// 3. Queries StateActor for the CC value
    /// 4. Transforms CC (7-bit) to PB (14-bit) using fast approximation
    ///
    /// Returns transformed PB entry if CC value found, None otherwise
    async fn try_cc_to_pb_transform(
        &self,
        page: &PageConfig,
        app: &AppKey,
        pb_channel: u8,
    ) -> Option<MidiStateEntry> {
        use crate::control_mapping::{load_default_mappings, MidiSpec};

        trace!(
            "CC->PB transform: app={:?} page={} pb_channel={}",
            app,
            page.name,
            pb_channel
        );

        // 1. Reverse lookup: Find control ID for this PB channel (e.g., "fader1" for ch1)
        let mapping_db = match load_default_mappings() {
            Ok(db) => db,
            Err(e) => {
                trace!("Failed to load mapping DB: {}", e);
                return None;
            },
        };

        let control_id = mapping_db
            .mappings
            .iter()
            .find(|(_, mapping)| {
                if let Ok(spec) = MidiSpec::parse(&mapping.mcu_message) {
                    matches!(spec, MidiSpec::PitchBend { channel } if channel == pb_channel.saturating_sub(1))
                } else {
                    false
                }
            })
            .map(|(id, _)| id.clone());

        let control_id = match control_id {
            Some(id) => {
                trace!("  Found control_id: {}", id);
                id
            },
            None => {
                trace!("  No control found for PB channel {}", pb_channel);
                return None;
            },
        };

        // 2. Get control config from page OR pages_global
        // First check page-specific controls, then fall back to global controls
        let control_config = {
            // Try page controls first
            let from_page = page.controls.as_ref().and_then(|c| c.get(&control_id));

            if let Some(config) = from_page {
                trace!(
                    "  Found control config for {} in page: app={}",
                    control_id,
                    config.app
                );
                config.clone()
            } else {
                // Fall back to global controls
                let config_guard = self.config.try_read().expect("Config lock poisoned");
                let from_global = config_guard
                    .pages_global
                    .as_ref()
                    .and_then(|g| g.controls.as_ref())
                    .and_then(|c| c.get(&control_id))
                    .cloned();
                drop(config_guard);

                match from_global {
                    Some(config) => {
                        trace!(
                            "  Found control config for {} in pages_global: app={}",
                            control_id,
                            config.app
                        );
                        config
                    },
                    None => {
                        trace!(
                            "  Control '{}' not found in page or pages_global",
                            control_id
                        );
                        return None;
                    },
                }
            }
        };

        // Ensure control's app matches the app we're querying for
        if control_config.app != app.as_str() {
            trace!(
                "  Control app '{}' doesn't match queried app '{:?}'",
                control_config.app,
                app
            );
            return None;
        }

        // 3. Check if control has CC mapping (not PB passthrough)
        let midi_spec = match control_config.midi.as_ref() {
            Some(spec) => spec,
            None => {
                trace!("  Control has no MIDI spec");
                return None;
            },
        };

        if !matches!(midi_spec.midi_type, crate::config::MidiType::Cc) {
            trace!(
                "  Control MIDI type is not CC (is {:?})",
                midi_spec.midi_type
            );
            return None;
        }

        // 4. Query StateActor for CC value (now async)
        let cc_channel = match midi_spec.channel {
            Some(ch) => ch,
            None => {
                trace!("  CC spec has no channel");
                return None;
            },
        };
        let cc_num = match midi_spec.cc {
            Some(num) => num,
            None => {
                trace!("  CC spec has no cc number");
                return None;
            },
        };

        trace!(
            "  Querying StateActor: app={:?} CC ch={} cc={}",
            app,
            cc_channel,
            cc_num
        );

        let cc_entry = match self
            .state_actor
            .get_known_latest(*app, MidiStatus::CC, Some(cc_channel), Some(cc_num))
            .await
        {
            Some(entry) => {
                trace!("  Found CC entry: value={:?}", entry.value);
                entry
            },
            None => {
                trace!("  No CC entry found in StateActor");
                return None;
            },
        };

        // 5. Transform CC (7-bit) to PB (14-bit)
        // Use proper linear scaling via centralized conversion
        let cc_value = match cc_entry.value.as_number() {
            Some(num) => num as u8,
            None => {
                trace!("  CC value is not a number");
                return None;
            },
        };
        let pb_value = crate::midi::convert::to_14bit(cc_value);

        trace!(
            "  Transform CC {} -> PB {} (0x{:04X})",
            cc_value,
            pb_value,
            pb_value
        );

        // 6. Create transformed PB entry
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

        let mut note_plan: HashMap<String, PlanEntry> = HashMap::new();
        let mut cc_plan: HashMap<String, PlanEntry> = HashMap::new();
        let mut pb_plan: HashMap<u8, PlanEntry> = HashMap::new();

        // Helper to push candidates with priority
        let push_note = |map: &mut HashMap<String, PlanEntry>, e: MidiStateEntry, prio: u8| {
            let key = format!(
                "{}|{}",
                e.addr.channel.unwrap_or(0),
                e.addr.data1.unwrap_or(0)
            );
            let should_insert = match map.get(&key) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(
                    key,
                    PlanEntry {
                        entry: e,
                        priority: prio,
                    },
                );
            }
        };

        let push_cc = |map: &mut HashMap<String, PlanEntry>, e: MidiStateEntry, prio: u8| {
            let key = format!(
                "{}|{}",
                e.addr.channel.unwrap_or(0),
                e.addr.data1.unwrap_or(0)
            );
            let should_insert = match map.get(&key) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(
                    key,
                    PlanEntry {
                        entry: e,
                        priority: prio,
                    },
                );
            }
        };

        let push_pb = |map: &mut HashMap<u8, PlanEntry>, ch: u8, e: MidiStateEntry, prio: u8| {
            let should_insert = match map.get(&ch) {
                None => true,
                Some(cur) => prio > cur.priority || (prio == cur.priority && e.ts > cur.entry.ts),
            };
            if should_insert {
                map.insert(
                    ch,
                    PlanEntry {
                        entry: e,
                        priority: prio,
                    },
                );
            }
        };

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
            // Note: Fader setpoint and zero fallbacks are handled AFTER all apps,
            // so apps don't overwrite each other's values
            for &ch in &channels {
                // Priority 3: Try to get known PB value (async query)
                if let Some(latest_pb) = self
                    .state_actor
                    .get_known_latest(*app, MidiStatus::PB, Some(ch), Some(0))
                    .await
                {
                    push_pb(&mut pb_plan, ch, latest_pb, 3);
                    continue;
                }

                // Priority 2: Try to transform CC to PB (for apps like QLC+, now async)
                if let Some(transformed_pb) = self.try_cc_to_pb_transform(_page, app, ch).await {
                    trace!(
                        "  Adding CC->PB to plan: ch={} value={:?}",
                        ch,
                        transformed_pb.value
                    );
                    push_pb(&mut pb_plan, ch, transformed_pb, 2);
                    continue;
                }

                // Don't fall back to setpoint or zero here - let other apps try first
            }

            // Notes: 0-31 - Always send Note Off to clear previous page buttons
            // Then let drivers send feedback to turn on buttons that should be ON
            for &ch in &channels {
                for note in 0..=31 {
                    let off = MidiStateEntry {
                        addr: MidiAddr {
                            port_id: app.as_str().to_string(),
                            status: MidiStatus::Note,
                            channel: Some(ch),
                            data1: Some(note),
                        },
                        value: MidiValue::Number(0),
                        ts: Self::now_ms(),
                        origin: Origin::XTouch,
                        known: false,
                        stale: false,
                        hash: None,
                    };
                    push_note(&mut note_plan, off, 1);
                }
            }

            // CC (rings): 0-31 - Always send 0 to clear previous page
            for &ch in &channels {
                for cc in 0..=31 {
                    let zero = MidiStateEntry {
                        addr: MidiAddr {
                            port_id: app.as_str().to_string(),
                            status: MidiStatus::CC,
                            channel: Some(ch),
                            data1: Some(cc),
                        },
                        value: MidiValue::Number(0),
                        ts: Self::now_ms(),
                        origin: Origin::XTouch,
                        known: false,
                        stale: false,
                        hash: None,
                    };
                    push_cc(&mut cc_plan, zero, 1);
                }
            }
        }

        // After all apps have been processed, fill in any missing fader channels
        // with fader setpoint (if available) or zero as fallback
        for &ch in &channels {
            // Skip if this channel already has an entry from an app
            if pb_plan.contains_key(&ch) {
                continue;
            }

            // Priority 2: Try to get from fader setpoint (motor position)
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
                push_pb(&mut pb_plan, ch, setpoint_pb, 2);
                continue;
            }

            // Fallback: send zero
            let zero_pb = MidiStateEntry {
                addr: MidiAddr {
                    port_id: "xtouch".to_string(),
                    status: MidiStatus::PB,
                    channel: Some(ch),
                    data1: Some(0),
                },
                value: MidiValue::Number(0),
                ts: Self::now_ms(),
                origin: Origin::XTouch,
                known: false,
                stale: false,
                hash: None,
            };
            push_pb(&mut pb_plan, ch, zero_pb, 1);
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
